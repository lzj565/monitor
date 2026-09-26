//! ProxyNode 创建时在 Hub 生成客户端与 Reality 所需身份材料。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use zeroize::Zeroize;

use crate::db::{Db, ProxyNode, ProxyNodeConfig};
use crate::proxy_config::validate_reality_settings;
use anyhow::Result;

const AUTO_PROXY_PORT_MIN: u16 = 24_000;
const AUTO_PROXY_PORT_MAX: u16 = 29_999;

/// 前端只提交代理节点的业务设置；密钥、UUID 和自动端口由 Hub 生成。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyNodeProvisionRequest {
    pub node_id: i64,
    pub name: String,
    pub address_mode: String,
    #[serde(default)]
    pub custom_address: Option<String>,
    #[serde(default)]
    pub listen_port: Option<u16>,
    pub reality_server_name: String,
    pub reality_dest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegenerateProxyNodeRequest {
    pub credential: ProxyCredential,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyCredential {
    Uuid,
    RealityKey,
    ShortId,
}

impl ProxyNodeProvisionRequest {
    pub fn validate(&self) -> Result<()> {
        if self.node_id <= 0 {
            refuse!("请选择所属服务器");
        }
        if self.name.trim().is_empty() {
            refuse!("请填写代理节点名称");
        }
        if !matches!(self.address_mode.as_str(), "ipv4" | "ipv6" | "custom") {
            refuse!("地址模式只支持 IPv4、IPv6 或自定义");
        }
        let custom_address_set =
            self.custom_address.as_deref().is_some_and(|address| !address.trim().is_empty());
        if self.address_mode == "custom" && !custom_address_set {
            refuse!("自定义地址模式必须填写连接地址");
        }
        if self.address_mode != "custom" && custom_address_set {
            refuse!("只有自定义地址模式可以填写连接地址");
        }
        if self.listen_port == Some(0) {
            refuse!("监听端口必须在 1 到 65535 之间");
        }
        validate_reality_settings(&self.reality_server_name, &self.reality_dest)
            .map_err(|error| anyhow::Error::msg(crate::Shown(error.to_string())))?;
        Ok(())
    }
}

pub fn create_proxy_node(db: &Db, request: &ProxyNodeProvisionRequest) -> Result<ProxyNode> {
    request.validate()?;
    let (private_key, public_key) = reality_keypair();
    let listen_port = request.listen_port.unwrap_or(1);
    let config = ProxyNodeConfig {
        node_id: request.node_id,
        name: request.name.trim().to_owned(),
        enabled: true,
        protocol: "vless_reality".into(),
        address_mode: request.address_mode.clone(),
        custom_address: request
            .custom_address
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(str::to_owned),
        listen_port,
        uuid: uuid_v4(),
        reality_private_key: private_key,
        reality_public_key: public_key,
        reality_short_id: hex::encode(rand::random::<[u8; 4]>()),
        reality_server_name: request.reality_server_name.trim().to_owned(),
        reality_dest: request.reality_dest.trim().to_owned(),
    };

    match request.listen_port {
        Some(_) => db.create_proxy_node(&config),
        None => db.create_proxy_node_auto_port(
            &config,
            rand::random_range(AUTO_PROXY_PORT_MIN..=AUTO_PROXY_PORT_MAX),
        ),
    }
}

pub fn regenerate_proxy_node(db: &Db, id: i64, credential: ProxyCredential) -> Result<bool> {
    match credential {
        ProxyCredential::Uuid => {
            let uuid = uuid_v4();
            db.regenerate_proxy_node_credentials(id, Some(&uuid), None, None, None)
        }
        ProxyCredential::RealityKey => {
            let (mut private_key, public_key) = reality_keypair();
            let result =
                db.regenerate_proxy_node_credentials(id, None, Some(&private_key), Some(&public_key), None);
            private_key.zeroize();
            result
        }
        ProxyCredential::ShortId => {
            let short_id = hex::encode(rand::random::<[u8; 4]>());
            db.regenerate_proxy_node_credentials(id, None, None, None, Some(&short_id))
        }
    }
}

pub(crate) fn validate_singbox_status(status: &Value) -> std::result::Result<(), &'static str> {
    if status.get("installed").and_then(Value::as_bool) == Some(false) {
        return Err("sing-box 未安装");
    }
    if status.get("installed").and_then(Value::as_bool) != Some(true) {
        return Err("无法确认 sing-box 是否已安装");
    }
    let state_known = status
        .get("service_state_known")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| status.get("service_exists").and_then(Value::as_bool) == Some(true));
    if !state_known {
        return Err("无法确认 sing-box 运行状态");
    }
    if status.get("service_exists").and_then(Value::as_bool) != Some(true)
        || status.get("running").and_then(Value::as_bool) != Some(true)
    {
        return Err("sing-box 未运行");
    }
    Ok(())
}

fn reality_keypair() -> (String, String) {
    let private = x25519_dalek::StaticSecret::random();
    let public = x25519_dalek::PublicKey::from(&private);
    let mut private_bytes = private.to_bytes();
    let private_key = URL_SAFE_NO_PAD.encode(private_bytes);
    private_bytes.zeroize();
    (private_key, URL_SAFE_NO_PAD.encode(public.to_bytes()))
}

fn uuid_v4() -> String {
    let mut bytes = rand::random::<[u8; 16]>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &value[..8], &value[8..12], &value[12..16], &value[16..20], &value[20..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Node;

    fn request(node_id: i64) -> ProxyNodeProvisionRequest {
        ProxyNodeProvisionRequest {
            node_id,
            name: " HK Reality ".into(),
            address_mode: "ipv4".into(),
            custom_address: None,
            listen_port: None,
            reality_server_name: "www.apple.com".into(),
            reality_dest: "www.apple.com:443".into(),
        }
    }

    #[test]
    fn generates_v4_identity_and_a_matching_reality_keypair() {
        let db = Db::open(":memory:").unwrap();
        let server = db.create_node(&Node { name: "hk".into(), ..Node::default() }, "server-token").unwrap();
        let created = create_proxy_node(&db, &request(server)).unwrap();

        let uuid = created.uuid.replace('-', "");
        assert_eq!(uuid.len(), 32);
        assert_eq!(uuid.as_bytes()[12], b'4');
        assert!(matches!(uuid.as_bytes()[16], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(created.reality_short_id.len(), 8);
        assert!((AUTO_PROXY_PORT_MIN..=AUTO_PROXY_PORT_MAX).contains(&created.listen_port));
        assert_eq!(created.name, "HK Reality");
        assert!(created.enabled);
        assert_eq!(created.protocol, "vless_reality");

        let private_bytes: [u8; 32] =
            URL_SAFE_NO_PAD.decode(&created.reality_private_key).unwrap().try_into().unwrap();
        let private = x25519_dalek::StaticSecret::from(private_bytes);
        let public = x25519_dalek::PublicKey::from(&private);
        assert_eq!(URL_SAFE_NO_PAD.encode(public.to_bytes()), created.reality_public_key);
    }

    #[test]
    fn regenerating_reality_keys_changes_the_pair_and_preserves_other_credentials() {
        let db = Db::open(":memory:").unwrap();
        let server = db.create_node(&Node { name: "hk".into(), ..Node::default() }, "server-token").unwrap();
        let created = create_proxy_node(&db, &request(server)).unwrap();
        let old_uuid = created.uuid.clone();
        let old_short_id = created.reality_short_id.clone();
        let old_private = created.reality_private_key.clone();
        let old_public = created.reality_public_key.clone();

        assert!(regenerate_proxy_node(&db, created.id, ProxyCredential::RealityKey).unwrap());
        let updated = db.proxy_node(created.id).unwrap().unwrap();
        assert_ne!(updated.reality_private_key, old_private);
        assert_ne!(updated.reality_public_key, old_public);
        assert_eq!(updated.uuid, old_uuid);
        assert_eq!(updated.reality_short_id, old_short_id);
        let private_bytes: [u8; 32] =
            URL_SAFE_NO_PAD.decode(&updated.reality_private_key).unwrap().try_into().unwrap();
        let private = x25519_dalek::StaticSecret::from(private_bytes);
        let public = x25519_dalek::PublicKey::from(&private);
        assert_eq!(URL_SAFE_NO_PAD.encode(public.to_bytes()), updated.reality_public_key);
    }

    #[test]
    fn rejects_invalid_reality_target_and_unknown_client_generated_fields() {
        let mut request = request(1);
        request.reality_dest = "invalid".into();
        assert!(request.validate().is_err());
        assert!(serde_json::from_value::<ProxyNodeProvisionRequest>(serde_json::json!({
            "node_id": 1,
            "name": "Reality",
            "address_mode": "ipv4",
            "custom_address": null,
            "listen_port": null,
            "reality_server_name": "www.apple.com",
            "reality_dest": "www.apple.com:443",
            "uuid": "must-not-come-from-the-browser"
        }))
        .is_err());
    }

    #[test]
    fn requires_an_installed_and_running_singbox_service() {
        assert_eq!(validate_singbox_status(&serde_json::json!({"installed": false})), Err("sing-box 未安装"));
        assert_eq!(
            validate_singbox_status(
                &serde_json::json!({"installed": true, "service_exists": true, "running": false})
            ),
            Err("sing-box 未运行")
        );
        assert!(validate_singbox_status(&serde_json::json!({
            "installed": true,
            "service_exists": true,
            "running": true
        }))
        .is_ok());
        assert_eq!(
            validate_singbox_status(&serde_json::json!({
                "installed": true,
                "service_exists": false,
                "running": false,
                "service_state_known": false
            })),
            Err("无法确认 sing-box 运行状态")
        );
    }
}
