//! Typed desired-state inputs and the V1 sing-box configuration generator.

use std::collections::BTreeSet;
use std::net::IpAddr;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::{self, AuthorizationDraft, Db, ProxyDraft, UserDraft};

pub const MAX_GENERATED_CONFIG: usize = 1024 * 1024;
pub const DEFAULT_FLOW: &str = "xtls-rprx-vision";

#[derive(Debug)]
pub struct InputError {
    pub code: &'static str,
    pub message: String,
}

impl InputError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyRequest {
    /// Optional on updates so older clients keep the proxy on its current node.
    #[serde(default)]
    pub node_id: Option<i64>,
    pub name: String,
    #[serde(default)]
    pub include_node_name: bool,
    pub protocol: String,
    pub address_type: String,
    pub address: String,
    pub port: u16,
    pub enabled: bool,
    #[serde(default)]
    pub flow: Option<String>,
    pub config: VlessProxyConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VlessProxyConfig {
    pub reality: RealityConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RealityConfig {
    pub enabled: bool,
    pub server_name: String,
    pub server_port: u16,
    pub private_key: String,
    pub public_key: String,
    pub short_id: String,
}

impl ProxyRequest {
    pub fn into_draft(self) -> Result<ProxyDraft, InputError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 128 {
            return Err(InputError::new("INVALID_PROXY_CONFIG", "name must contain 1 to 128 characters"));
        }
        if self.protocol != "vless" {
            return Err(InputError::new("INVALID_PROXY_PROTOCOL", "V1 only supports the vless protocol"));
        }
        validate_address(&self.address_type, &self.address)?;
        if self.port == 0 {
            return Err(InputError::new("INVALID_PROXY_CONFIG", "port must be between 1 and 65535"));
        }
        if self.flow.as_deref().is_some_and(|flow| !flow.is_empty() && flow != DEFAULT_FLOW) {
            return Err(InputError::new("INVALID_PROXY_CONFIG", "flow must be empty or xtls-rprx-vision"));
        }
        let reality = &self.config.reality;
        if !reality.enabled {
            return Err(InputError::new("INVALID_PROXY_CONFIG", "V1 requires VLESS Reality to be enabled"));
        }
        if !valid_host(&reality.server_name) || reality.server_port == 0 {
            return Err(InputError::new(
                "INVALID_PROXY_CONFIG",
                "Reality requires a valid handshake server and port",
            ));
        }
        if !valid_reality_key(&reality.private_key) || !valid_reality_key(&reality.public_key) {
            return Err(InputError::new(
                "INVALID_PROXY_CONFIG",
                "Reality keys must be 43 character base64url keys",
            ));
        }
        if reality.short_id.len() > 8 || !reality.short_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(InputError::new(
                "INVALID_PROXY_CONFIG",
                "Reality short_id must be up to 8 hexadecimal characters",
            ));
        }
        let config = serde_json::to_value(self.config)
            .map_err(|e| InputError::new("INVALID_PROXY_CONFIG", e.to_string()))?;
        let encoded = serde_json::to_vec(&config)
            .map_err(|e| InputError::new("INVALID_PROXY_CONFIG", e.to_string()))?;
        if encoded.len() > 16 * 1024 {
            return Err(InputError::new("INVALID_PROXY_CONFIG", "proxy config exceeds 16 KiB"));
        }
        Ok(ProxyDraft {
            name: name.to_owned(),
            include_node_name: self.include_node_name,
            protocol: self.protocol,
            address_type: self.address_type,
            address: self.address,
            port: i64::from(self.port),
            enabled: self.enabled,
            flow: self.flow,
            config,
        })
    }
}

fn validate_address(address_type: &str, address: &str) -> Result<(), InputError> {
    let valid = match address_type {
        "ipv4" => address.parse::<IpAddr>().is_ok_and(|a| a.is_ipv4()),
        "ipv6" => address.parse::<IpAddr>().is_ok_and(|a| a.is_ipv6()),
        "domain" => valid_host(address),
        _ => {
            return Err(InputError::new("INVALID_ADDRESS_TYPE", "address_type must be ipv4, ipv6, or domain"))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(InputError::new("INVALID_ADDRESS", "address does not match address_type"))
    }
}

fn valid_host(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok() || valid_domain(host)
}

fn valid_domain(domain: &str) -> bool {
    domain.len() <= 253
        && !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && label.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn valid_reality_key(value: &str) -> bool {
    value.len() == 43 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserRequest {
    pub username: String,
    #[serde(default)]
    pub password: Option<String>,
    pub enabled: bool,
    pub expires_at: Option<i64>,
}

impl UserRequest {
    pub fn into_draft(self, creating: bool) -> Result<UserDraft, InputError> {
        let username = self.username.trim();
        if username.is_empty() || username.chars().count() > 128 {
            return Err(InputError::new("INVALID_USER", "username must contain 1 to 128 characters"));
        }
        let password_hash = match self.password {
            Some(password) if password.is_empty() && !creating => None,
            Some(password) if password.is_empty() => {
                return Err(InputError::new("PASSWORD_REQUIRED", "请设置用户登录密码"));
            }
            Some(password) if password.len() > 1024 => {
                return Err(InputError::new("INVALID_PASSWORD", "密码不能超过 1024 字节"));
            }
            Some(password) => Some(
                crate::auth::hash_password(&password)
                    .map_err(|_| InputError::new("PASSWORD_HASH_FAILED", "无法设置用户密码"))?,
            ),
            None if creating => return Err(InputError::new("PASSWORD_REQUIRED", "请设置用户登录密码")),
            None => None,
        };
        Ok(UserDraft {
            username: username.to_owned(),
            password_hash,
            enabled: self.enabled,
            expires_at: self.expires_at,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VlessAuthorizationSettings {
    #[serde(default)]
    pub flow: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRequest {
    pub enabled: bool,
    #[serde(default)]
    pub auth: Option<VlessAuthorizationSettings>,
}

impl AuthorizationRequest {
    pub fn into_draft(self) -> Result<AuthorizationDraft, InputError> {
        if self.auth.as_ref().is_some_and(|auth| !auth.flow.is_empty() && auth.flow != "xtls-rprx-vision") {
            return Err(InputError::new("INVALID_PROXY_AUTH", "auth.flow must be empty or xtls-rprx-vision"));
        }
        let auth = self
            .auth
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| InputError::new("INVALID_PROXY_AUTH", e.to_string()))?;
        Ok(AuthorizationDraft { enabled: self.enabled, auth })
    }
}

pub fn generate_config(db: &Db, node_id: i64, now: i64) -> anyhow::Result<Value> {
    let node = db.node(node_id)?.with_context(|| format!("node {node_id} not found"))?;
    let listen = if node.ipv6.is_empty() { "0.0.0.0" } else { "::" };
    let mut inbounds = Vec::new();
    let mut stats_inbounds = Vec::new();
    let mut stats_users = BTreeSet::new();
    for proxy in db.proxies_for_node(node_id)?.into_iter().filter(|p| p.enabled) {
        let config: VlessProxyConfig = serde_json::from_value(proxy.config.clone())
            .with_context(|| format!("stored VLESS config for proxy {} is invalid", proxy.id))?;
        let users: Vec<Value> = db
            .active_users_for_proxy(proxy.id, now)?
            .into_iter()
            .map(|auth| {
                let auth: VlessUser = serde_json::from_value(auth)
                    .with_context(|| format!("stored VLESS auth for proxy {} is invalid", proxy.id))?;
                stats_users.insert(auth.name.clone());
                let mut user = json!({"name": auth.name, "uuid": auth.uuid});
                user["flow"] = Value::String(if proxy.flow.is_empty() {
                    DEFAULT_FLOW.to_owned()
                } else {
                    proxy.flow.clone()
                });
                Ok(user)
            })
            .collect::<anyhow::Result<_>>()?;
        let reality = config.reality;
        inbounds.push(json!({
            "type": "vless",
            "tag": format!("proxy-{}", proxy.id),
            "listen": listen,
            "listen_port": proxy.port,
            "users": users,
            "tls": {
                "enabled": true,
                "server_name": reality.server_name,
                "reality": {
                    "enabled": true,
                    "handshake": {
                        "server": reality.server_name,
                        "server_port": reality.server_port
                    },
                    "private_key": reality.private_key,
                    "short_id": [reality.short_id]
                }
            }
        }));
        stats_inbounds.push(format!("proxy-{}", proxy.id));
    }
    let config = json!({
        "log": {"level": "warn"},
        "dns": {
            "final": "dns-local",
            "servers": [{"prefer_go": true, "tag": "dns-local", "type": "local"}],
            "strategy": "prefer_ipv4"
        },
        "inbounds": inbounds,
        "outbounds": [{"type": "direct", "tag": "direct"}],
        "experimental": {
            "v2ray_api": {
                "stats": {
                    "enabled": true,
                    "inbounds": stats_inbounds,
                    "users": stats_users.into_iter().collect::<Vec<_>>()
                }
            }
        }
    });
    let size = serde_json::to_vec(&config)?.len();
    anyhow::ensure!(size <= MAX_GENERATED_CONFIG, "generated config exceeds 1 MiB");
    Ok(config)
}

#[derive(Debug, Deserialize)]
struct VlessUser {
    name: String,
    uuid: String,
}

pub fn vless_protocol(proxy: &db::Proxy) -> bool {
    proxy.protocol == "vless"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Node, UserDraft};

    #[test]
    fn custom_addresses_allow_domains_and_ip_literals() {
        assert!(validate_address("domain", "hk.example.com").is_ok());
        assert!(validate_address("domain", "82.29.36.116").is_ok());
        assert!(validate_address("domain", "2400:xxxx::1").is_err());
        assert!(validate_address("domain", "2400:db8::1").is_ok());
    }

    fn request_json(include_node_name: Option<bool>) -> Value {
        let mut request = json!({
            "name": " Reality ",
            "protocol": "vless",
            "address_type": "domain",
            "address": "hk.example.com",
            "port": 24060,
            "enabled": true,
            "config": {"reality": {
                "enabled": true,
                "server_name": "www.amd.com",
                "server_port": 443,
                "private_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "public_key": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
                "short_id": "abcdef12"
            }}
        });
        if let Some(include) = include_node_name {
            request["include_node_name"] = json!(include);
        }
        request
    }

    #[test]
    fn proxy_request_keeps_the_node_name_option_and_defaults_old_clients_off() {
        let old: ProxyRequest = serde_json::from_value(request_json(None)).unwrap();
        assert_eq!(old.node_id, None);
        let old = old.into_draft().unwrap();
        assert_eq!(old.name, "Reality");
        assert!(!old.include_node_name);
        assert_eq!(old.flow, None);

        let current: ProxyRequest = serde_json::from_value(request_json(Some(true))).unwrap();
        assert!(current.into_draft().unwrap().include_node_name);

        let mut update = request_json(None);
        update["node_id"] = json!(42);
        let update: ProxyRequest = serde_json::from_value(update).unwrap();
        assert_eq!(update.node_id, Some(42));
    }

    #[test]
    fn proxy_flow_accepts_only_supported_values() {
        let mut request = request_json(None);
        request["flow"] = json!("xtls-rprx-vision");
        let draft = serde_json::from_value::<ProxyRequest>(request.clone()).unwrap().into_draft().unwrap();
        assert_eq!(draft.flow.as_deref(), Some("xtls-rprx-vision"));
        request["flow"] = json!("unsupported");
        assert!(serde_json::from_value::<ProxyRequest>(request).unwrap().into_draft().is_err());
    }

    #[test]
    fn authorization_writes_can_omit_per_user_auth_settings() {
        let request: AuthorizationRequest = serde_json::from_value(json!({"enabled": true})).unwrap();
        let draft = request.into_draft().unwrap();
        assert!(draft.enabled);
        assert!(draft.auth.is_none());

        let legacy: AuthorizationRequest = serde_json::from_value(json!({
            "enabled": true,
            "auth": {"flow": "xtls-rprx-vision"}
        }))
        .unwrap();
        assert_eq!(legacy.into_draft().unwrap().auth.unwrap()["flow"], "xtls-rprx-vision");
    }

    #[test]
    fn user_password_is_required_on_create_and_optional_on_update() {
        let request = |password: Option<&str>| {
            let mut value = json!({"username":" client ","enabled":true,"expires_at":null});
            if let Some(password) = password {
                value["password"] = json!(password);
            }
            serde_json::from_value::<UserRequest>(value).unwrap()
        };

        assert!(request(None).into_draft(true).is_err());
        assert!(request(Some("")).into_draft(true).is_err());
        let created = request(Some("secret")).into_draft(true).unwrap();
        assert!(created.password_hash.as_deref().is_some_and(|hash| hash.starts_with("$argon2")));
        assert_eq!(request(None).into_draft(false).unwrap().password_hash, None);
        assert_eq!(request(Some("")).into_draft(false).unwrap().password_hash, None);
        assert!(request(Some("secret")).into_draft(false).unwrap().password_hash.is_some());
        assert!(
            serde_json::from_value::<UserRequest>(json!({
                "username":"client", "uuid":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "enabled":true,"expires_at":null,"password":"secret"
            }))
            .is_err(),
            "UUID is not accepted in the generic user write API"
        );
    }

    #[test]
    fn sing_box_uses_the_same_user_uuid_on_authorized_proxies() {
        let db = Db::open(":memory:").unwrap();
        let user_id = db
            .create_user(&UserDraft {
                username: "same".into(),
                password_hash: Some("hash".into()),
                enabled: true,
                expires_at: None,
            })
            .unwrap()
            .unwrap();
        let mut proxy_ids = Vec::new();
        let mut node_ids = Vec::new();
        for (name, port) in [("HK", 24060), ("JP", 24061)] {
            let node_id = db
                .create_node(&Node { name: name.into(), ..Default::default() }, &format!("token-{name}"))
                .unwrap();
            let proxy_id = db.create_proxy(node_id, &db::ProxyDraft {
                name: name.into(), include_node_name: false, protocol: "vless".into(), address_type: "domain".into(),
                address: "example.com".into(), port, enabled: true,
                flow: (port != 24060).then(|| DEFAULT_FLOW.to_owned()),
                config: json!({"reality":{"enabled":true,"server_name":"example.com","server_port":443,
                    "private_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","public_key":"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB","short_id":"abcdef12"}}),
            }).unwrap().unwrap();
            // The legacy authorization value is deliberately different: proxy
            // Flow is now the single source used by generated node configs.
            let draft = AuthorizationDraft { enabled: true, auth: Some(json!({"flow":""})) };
            db.put_authorization(user_id, proxy_id, &draft).unwrap().unwrap();
            proxy_ids.push(proxy_id);
            node_ids.push(node_id);
        }
        let uuid = db.user(user_id).unwrap().unwrap().uuid;
        for node_id in node_ids {
            let config = generate_config(&db, node_id, chrono::Utc::now().timestamp()).unwrap();
            assert_eq!(config["inbounds"][0]["users"][0]["uuid"], uuid);
            assert_eq!(config["inbounds"][0]["users"][0]["name"], "same");
            assert_eq!(config["inbounds"][0]["users"][0]["flow"], "xtls-rprx-vision");
            for section in ["experimental", "dns", "inbounds", "log", "outbounds"] {
                assert!(config.get(section).is_some(), "missing required section: {section}");
            }
            assert_eq!(config["log"]["level"], "warn");
            assert_eq!(config["dns"]["final"], "dns-local");
            assert_eq!(config["dns"]["strategy"], "prefer_ipv4");
            assert_eq!(config["dns"]["servers"][0]["type"], "local");
            assert_eq!(config["dns"]["servers"][0]["tag"], "dns-local");
            assert_eq!(config["dns"]["servers"][0]["prefer_go"], true);
            assert_eq!(config["inbounds"][0]["listen"], "0.0.0.0");
            assert_eq!(config["inbounds"][0]["tls"]["server_name"], "example.com");
            assert_eq!(
                config["inbounds"][0]["tls"]["server_name"],
                config["inbounds"][0]["tls"]["reality"]["handshake"]["server"]
            );
            assert!(config["experimental"]["v2ray_api"].get("listen").is_none());
            assert_eq!(config["experimental"]["v2ray_api"]["stats"]["enabled"], true);
            assert_eq!(
                config["experimental"]["v2ray_api"]["stats"]["inbounds"][0],
                config["inbounds"][0]["tag"]
            );
            assert_eq!(config["experimental"]["v2ray_api"]["stats"]["users"][0], "same");
        }
        assert_eq!(proxy_ids.len(), 2);
    }

    #[test]
    fn generated_config_keeps_the_local_api_enabled_without_proxies() {
        let db = Db::open(":memory:").unwrap();
        let node_id = db.create_node(&Node { name: "empty".into(), ..Default::default() }, "token").unwrap();
        let config = generate_config(&db, node_id, chrono::Utc::now().timestamp()).unwrap();
        for section in ["experimental", "dns", "inbounds", "log", "outbounds"] {
            assert!(config.get(section).is_some(), "missing required section: {section}");
        }
        assert_eq!(config["log"]["level"], "warn");
        assert_eq!(config["inbounds"], json!([]));
        assert!(config["experimental"]["v2ray_api"].get("listen").is_none());
        assert_eq!(config["experimental"]["v2ray_api"]["stats"]["enabled"], true);
        assert_eq!(config["experimental"]["v2ray_api"]["stats"]["inbounds"], json!([]));
        assert_eq!(config["experimental"]["v2ray_api"]["stats"]["users"], json!([]));
    }

    #[test]
    fn generated_config_uses_ipv6_wildcard_when_node_reports_ipv6() {
        let db = Db::open(":memory:").unwrap();
        let node_id = db.create_node(&Node { name: "ipv6".into(), ..Default::default() }, "token").unwrap();
        db.save_facts(node_id, &json!({"ipv6":"2001:db8::1"}), "198.51.100.1", "198.51.100.1").unwrap();
        let proxy_id = db.create_proxy(node_id, &db::ProxyDraft {
            name: "ipv6".into(), include_node_name: false, protocol: "vless".into(), address_type: "domain".into(),
            address: "example.com".into(), port: 24062, enabled: true, flow: None,
            config: json!({"reality":{"enabled":true,"server_name":"example.com","server_port":443,
                "private_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","public_key":"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB","short_id":"abcdef12"}}),
        }).unwrap().unwrap();
        let config = generate_config(&db, node_id, chrono::Utc::now().timestamp()).unwrap();
        assert_eq!(config["inbounds"][0]["tag"], format!("proxy-{proxy_id}"));
        assert_eq!(config["inbounds"][0]["listen"], "::");
    }
}
