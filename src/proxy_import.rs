//! 读取并校验服务器上可接管的 VLESS + Reality inbound。

use std::collections::HashMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::proxy_config::{
    is_reality_short_id, is_uuid, validate_reality_settings, MANAGED_INBOUND_TAG_PREFIX,
};
use crate::proxy_provision::reality_public_key_from_private;

#[derive(Serialize, Clone)]
pub struct ProxyImportCandidate {
    pub source_tag: Option<String>,
    pub importable: bool,
    pub reason: Option<String>,
    pub name: String,
    pub protocol: String,
    pub listen_address: Option<String>,
    pub listen_port: Option<u16>,
    pub uuid: Option<String>,
    pub reality_public_key: Option<String>,
    pub reality_short_id: Option<String>,
    pub reality_server_name: Option<String>,
    pub reality_dest: Option<String>,
    pub suggested_address_mode: Option<String>,
    pub suggested_address: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyImportRequest {
    pub config_fingerprint: String,
    pub source_tag: String,
    pub name: String,
    pub address_mode: String,
    #[serde(default)]
    pub custom_address: Option<String>,
}

/// 只在 Hub 内存中短暂保存解析出的私钥，不实现 Serialize 或 Debug。
pub struct ImportedInbound {
    pub source_tag: String,
    pub listen_address: String,
    pub listen_port: u16,
    pub uuid: String,
    pub reality_private_key: String,
    pub reality_public_key: String,
    pub reality_short_id: String,
    pub reality_server_name: String,
    pub reality_dest: String,
}

struct InspectedInbound {
    preview: ProxyImportCandidate,
    imported: Option<ImportedInbound>,
}

pub fn scan_config(config: &Value) -> Result<Vec<ProxyImportCandidate>, &'static str> {
    let inbounds = match config.get("inbounds") {
        None => return Ok(Vec::new()),
        Some(Value::Array(inbounds)) => inbounds,
        Some(_) => return Err("sing-box 配置中的 inbounds 必须是数组"),
    };

    let mut inspected = Vec::new();
    for inbound in inbounds {
        if inbound
            .get("tag")
            .and_then(Value::as_str)
            .is_some_and(|tag| tag.starts_with(MANAGED_INBOUND_TAG_PREFIX))
        {
            continue;
        }
        inspected.push(inspect_inbound(inbound));
    }

    let mut tag_counts = HashMap::<String, usize>::new();
    for entry in &inspected {
        if let Some(tag) = &entry.preview.source_tag {
            *tag_counts.entry(tag.clone()).or_default() += 1;
        }
    }

    Ok(inspected
        .into_iter()
        .map(|mut entry| {
            if entry
                .preview
                .source_tag
                .as_ref()
                .is_some_and(|tag| tag_counts.get(tag).copied().unwrap_or(0) > 1)
            {
                entry.preview.importable = false;
                entry.preview.reason = Some("配置中存在重复 tag，无法安全接管".into());
            }
            entry.preview
        })
        .collect())
}

pub fn find_importable(config: &Value, source_tag: &str) -> Result<ImportedInbound, String> {
    let inbounds = match config.get("inbounds") {
        None => return Err("配置已变化，请重新扫描".into()),
        Some(Value::Array(inbounds)) => inbounds,
        Some(_) => return Err("sing-box 配置中的 inbounds 必须是数组".into()),
    };
    let matches = inbounds
        .iter()
        .filter(|inbound| inbound.get("tag").and_then(Value::as_str) == Some(source_tag))
        .collect::<Vec<_>>();
    let [inbound] = matches.as_slice() else {
        return Err("配置已变化，请重新扫描".into());
    };
    let inspected = inspect_inbound(inbound);
    match inspected.imported {
        Some(imported) => Ok(imported),
        None => Err(inspected.preview.reason.unwrap_or_else(|| "该 inbound 当前不可导入".into())),
    }
}

fn inspect_inbound(inbound: &Value) -> InspectedInbound {
    let tag =
        inbound.get("tag").and_then(Value::as_str).filter(|tag| !tag.trim().is_empty()).map(str::to_owned);
    let kind = inbound.get("type").and_then(Value::as_str).unwrap_or("未知");
    let port = inbound.get("listen_port").and_then(Value::as_u64).and_then(|port| u16::try_from(port).ok());
    let user = inbound.get("users").and_then(Value::as_array).and_then(|users| users.first());
    let uuid = user.and_then(|user| user.get("uuid")).and_then(Value::as_str).map(str::to_owned);
    let reality = inbound.get("tls").and_then(|tls| tls.get("reality"));
    let private_key = reality.and_then(|reality| reality.get("private_key")).and_then(Value::as_str);
    let public_key = private_key.and_then(|key| reality_public_key_from_private(key).ok());
    let short_id = reality
        .and_then(|reality| reality.get("short_id"))
        .and_then(Value::as_array)
        .and_then(|ids| (ids.len() == 1).then(|| ids.first()).flatten())
        .and_then(Value::as_str)
        .map(str::to_owned);
    let server_name =
        inbound.get("tls").and_then(|tls| tls.get("server_name")).and_then(Value::as_str).map(str::to_owned);
    let dest = reality_dest(reality);
    let listen_address = inbound
        .get("listen")
        .and_then(Value::as_str)
        .and_then(|listen| listen.parse::<IpAddr>().ok())
        .map(|listen| listen.to_string());
    let mut preview = ProxyImportCandidate {
        source_tag: tag.clone(),
        importable: false,
        reason: None,
        name: tag.clone().unwrap_or_else(|| {
            port.map_or_else(|| "未命名 inbound".into(), |port| format!("Reality {port}"))
        }),
        protocol: if kind == "vless" { "VLESS".into() } else { kind.into() },
        listen_address: listen_address.clone(),
        listen_port: port,
        uuid: uuid.clone(),
        reality_public_key: public_key.clone(),
        reality_short_id: short_id.clone(),
        reality_server_name: server_name.clone(),
        reality_dest: dest.clone(),
        suggested_address_mode: None,
        suggested_address: None,
    };

    let imported = match parse_importable(
        inbound,
        tag.as_deref(),
        listen_address.as_deref(),
        port,
        uuid.as_deref(),
        private_key,
        public_key.as_deref(),
        short_id.as_deref(),
        server_name.as_deref(),
        dest.as_deref(),
    ) {
        Ok(imported) => {
            preview.importable = true;
            Some(imported)
        }
        Err(reason) => {
            preview.reason = Some(reason.into());
            None
        }
    };
    InspectedInbound { preview, imported }
}

#[allow(clippy::too_many_arguments)]
fn parse_importable(
    inbound: &Value,
    tag: Option<&str>,
    listen_address: Option<&str>,
    port: Option<u16>,
    uuid: Option<&str>,
    private_key: Option<&str>,
    public_key: Option<&str>,
    short_id: Option<&str>,
    server_name: Option<&str>,
    dest: Option<&str>,
) -> Result<ImportedInbound, &'static str> {
    if inbound.get("type").and_then(Value::as_str) != Some("vless") {
        return Err("非 VLESS inbound");
    }
    let tls = inbound.get("tls").ok_or("缺少 Reality TLS 配置")?;
    let reality = tls.get("reality").ok_or("非 Reality VLESS")?;
    if tls.get("enabled").and_then(Value::as_bool) != Some(true)
        || reality.get("enabled").and_then(Value::as_bool) != Some(true)
    {
        return Err("非 Reality VLESS");
    }
    let users = inbound.get("users").and_then(Value::as_array).ok_or("缺少 UUID")?;
    if users.len() != 1 {
        return Err("多用户 inbound 暂不支持导入");
    }
    let user = users.first().ok_or("缺少 UUID")?;
    let flow = user.get("flow");
    if flow.is_some_and(|flow| flow.as_str() != Some("xtls-rprx-vision")) {
        return Err("当前只支持 xtls-rprx-vision flow");
    }
    if !only_keys(inbound, &["type", "tag", "listen", "listen_port", "users", "tls"])
        || !only_keys(tls, &["enabled", "server_name", "reality"])
        || !only_keys(reality, &["enabled", "handshake", "private_key", "short_id"])
        || !only_keys(user, &["uuid", "flow", "name"])
        || reality.get("handshake").is_some_and(|handshake| !only_keys(handshake, &["server", "server_port"]))
    {
        return Err("包含当前 ProxyNode 暂不支持保留的配置字段");
    }
    let tag = tag.ok_or("缺少 tag，无法安全接管 inbound")?;
    let listen_address = listen_address.ok_or("listen 必须是 IPv4 或 IPv6 监听地址")?;
    let listen_port = port.filter(|port| *port != 0).ok_or("listen_port 无效")?;
    let uuid = uuid.ok_or("缺少 UUID")?;
    if !is_uuid(uuid) {
        return Err("UUID 格式无效");
    }
    let ids = reality.get("short_id").and_then(Value::as_array).ok_or("缺少 Reality Short ID")?;
    if ids.len() != 1 {
        return Err("多 Short ID 暂不支持导入");
    }
    let short_id = short_id.ok_or("Reality Short ID 无效")?;
    if !is_reality_short_id(short_id) {
        return Err("Reality Short ID 无效");
    }
    let private_key = private_key.ok_or("缺少 Reality private key")?;
    let public_key = public_key.ok_or("Reality private key 格式无效，无法推导 public key")?;
    let server_name = server_name.ok_or("缺少 Reality SNI")?;
    let dest = dest.ok_or("Dest 配置无法解析")?;
    validate_reality_settings(server_name, dest).map_err(|_| "Dest 或 SNI 格式无效")?;

    Ok(ImportedInbound {
        source_tag: tag.to_owned(),
        listen_address: listen_address.to_owned(),
        listen_port,
        uuid: uuid.to_owned(),
        reality_private_key: private_key.to_owned(),
        reality_public_key: public_key.to_owned(),
        reality_short_id: short_id.to_owned(),
        reality_server_name: server_name.to_owned(),
        reality_dest: dest.to_owned(),
    })
}

fn reality_dest(reality: Option<&Value>) -> Option<String> {
    let handshake = reality?.get("handshake")?;
    let server = handshake.get("server")?.as_str()?.trim();
    let port = handshake.get("server_port")?.as_u64().and_then(|port| u16::try_from(port).ok())?;
    if server.is_empty() || port == 0 {
        return None;
    }
    let host = match server.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{server}]"),
        _ => server.to_owned(),
    };
    Some(format!("{host}:{port}"))
}

fn only_keys(value: &Value, allowed: &[&str]) -> bool {
    value.as_object().is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inbound(tag: &str) -> Value {
        json!({
            "type": "vless",
            "tag": tag,
            "listen": "0.0.0.0",
            "listen_port": 33333,
            "tls": {
                "enabled": true,
                "server_name": "www.amd.com",
                "reality": {
                    "enabled": true,
                    "handshake": {"server": "www.amd.com", "server_port": 443},
                    "private_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                    "short_id": ["3efe85475d460e65"]
                }
            },
            "users": [{"flow": "xtls-rprx-vision", "name": "aaa", "uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e"}]
        })
    }

    #[test]
    fn scans_supported_inbounds_without_serializing_private_key() {
        let config = json!({"inbounds": [inbound("reality-hk"), {
            "type": "vless", "tag": "monitor-proxy-node-9"
        }]});
        let candidates = scan_config(&config).unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].importable);
        assert_eq!(candidates[0].source_tag.as_deref(), Some("reality-hk"));
        assert_eq!(candidates[0].listen_address.as_deref(), Some("0.0.0.0"));
        let serialized = serde_json::to_string(&candidates).unwrap();
        assert!(!serialized.contains("private_key"));
        assert!(!serialized.contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
        assert!(candidates[0].reality_public_key.is_some());
    }

    #[test]
    fn preserves_ipv6_wildcard_listener_and_accepts_an_unset_flow() {
        let mut value = inbound("ipv6-listener");
        value["listen"] = json!("::");
        value["users"][0].as_object_mut().unwrap().remove("flow");
        let candidate = scan_config(&json!({"inbounds": [value]})).unwrap().remove(0);

        assert!(candidate.importable);
        assert_eq!(candidate.listen_address.as_deref(), Some("::"));
        assert_eq!(candidate.suggested_address, None);
    }

    #[test]
    fn rejects_a_flow_other_than_vision() {
        let mut value = inbound("other-flow");
        value["users"][0]["flow"] = json!("xtls-rprx-splice");
        let candidate = scan_config(&json!({"inbounds": [value]})).unwrap().remove(0);

        assert_eq!(candidate.reason.as_deref(), Some("当前只支持 xtls-rprx-vision flow"));
    }

    #[test]
    fn reports_unsupported_shapes_without_failing_the_scan() {
        let mut multiple_users = inbound("multiple-users");
        multiple_users["users"] = json!([
            {"uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e"},
            {"uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8f"}
        ]);
        let mut multiple_ids = inbound("multiple-ids");
        multiple_ids["tls"]["reality"]["short_id"] = json!(["1234abcd", "5678abcd"]);
        let config = json!({"inbounds": [multiple_users, multiple_ids, {"type": "socks", "tag": "local"}]});
        let candidates = scan_config(&config).unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].reason.as_deref(), Some("多用户 inbound 暂不支持导入"));
        assert_eq!(candidates[1].reason.as_deref(), Some("多 Short ID 暂不支持导入"));
        assert_eq!(candidates[2].reason.as_deref(), Some("非 VLESS inbound"));
    }

    #[test]
    fn rejects_duplicate_tags_and_unpreservable_listen_addresses() {
        let mut ipv6 = inbound("duplicate");
        ipv6["listen"] = json!("::");
        let config = json!({"inbounds": [inbound("duplicate"), ipv6]});
        let candidates = scan_config(&config).unwrap();
        assert!(candidates.iter().all(|candidate| !candidate.importable));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.reason.as_deref() == Some("配置中存在重复 tag，无法安全接管")));

        let mut bad_listen = inbound("bad-listen");
        bad_listen["listen"] = json!("eth0");
        let candidate = scan_config(&json!({"inbounds": [bad_listen]})).unwrap().remove(0);
        assert_eq!(candidate.reason.as_deref(), Some("listen 必须是 IPv4 或 IPv6 监听地址"));
    }

    #[test]
    fn builds_bracketed_ipv6_dest_and_revalidates_the_selected_tag() {
        let mut value = inbound("ipv6-dest");
        value["tls"]["reality"]["handshake"]["server"] = json!("2001:db8::1");
        let config = json!({"inbounds": [value]});
        let found = find_importable(&config, "ipv6-dest").unwrap();
        assert_eq!(found.reality_dest, "[2001:db8::1]:443");
        assert!(find_importable(&config, "missing").is_err());
    }

    #[test]
    fn an_inbound_with_unmodeled_behavior_is_not_importable() {
        let mut value = inbound("unmodeled");
        value["sniff"] = json!(true);
        let candidate = scan_config(&json!({"inbounds": [value]})).unwrap().remove(0);
        assert_eq!(candidate.reason.as_deref(), Some("包含当前 ProxyNode 暂不支持保留的配置字段"));
    }
}
