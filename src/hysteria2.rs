//! Hysteria2 public settings, separate from server-only certificate material.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub server_name: String,
    #[serde(default)]
    pub obfs_password: String,
    #[serde(default = "default_bandwidth")]
    pub up_mbps: u32,
    #[serde(default = "default_bandwidth")]
    pub down_mbps: u32,
}
fn default_bandwidth() -> u32 {
    100
}
impl Settings {
    pub fn validate(&self) -> Result<(), &'static str> {
        let host = &self.server_name;
        if host.is_empty()
            || host.len() > 253
            || !host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err("invalid HY2 server name");
        }
        if self.up_mbps == 0 || self.down_mbps == 0 {
            return Err("HY2 bandwidth must be positive");
        }
        if self.obfs_password.len() > 1024 || self.obfs_password.chars().any(char::is_control) {
            return Err("invalid Salamander password");
        }
        Ok(())
    }
    pub fn inbound(
        &self,
        tag: &str,
        listen: &str,
        port: u16,
        users: Vec<Value>,
        certificate: &str,
        private_key: &str,
    ) -> Value {
        let mut result = json!({"type":"hysteria2", "tag":tag, "listen":listen,
            "listen_port":port, "users":users,
            "tls":{"enabled":true,"server_name":self.server_name,"alpn":["h3"],
                "certificate":[certificate],"key":[private_key]}});
        if !self.obfs_password.is_empty() {
            result["obfs"] = json!({"type":"salamander","password":self.obfs_password});
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_defaults_and_validation() {
        let mut settings: Settings = serde_json::from_value(json!({"server_name":"example.com"})).unwrap();
        assert_eq!(settings.up_mbps, 100);
        assert!(settings.validate().is_ok());
        settings.server_name = "bad/name".into();
        assert!(settings.validate().is_err());
        settings.server_name = "example.com".into();
        settings.down_mbps = 0;
        assert!(settings.validate().is_err());
    }
    #[test]
    fn inbound_keeps_authentication_and_optional_obfuscation() {
        let mut settings: Settings = serde_json::from_value(json!({"server_name":"example.com"})).unwrap();
        let users = vec![json!({"name":"alice","password":"user-uuid"})];
        let inbound = settings.inbound("proxy-1", "::", 443, users.clone(), "cert", "key");
        assert_eq!(inbound["users"], json!(users));
        assert!(inbound.get("obfs").is_none());
        settings.obfs_password = "secret".into();
        assert_eq!(
            settings.inbound("proxy-1", "::", 443, users, "cert", "key")["obfs"]["type"],
            "salamander"
        );
    }
}

/// Keep certificate material separate from user-facing settings.
pub fn persisted_config(settings: &Value, previous: Option<&Value>) -> anyhow::Result<Value> {
    let typed: Settings = serde_json::from_value(settings["hysteria2"].clone())?;
    typed.validate().map_err(anyhow::Error::msg)?;
    let mut config = serde_json::to_value(&typed)?;
    if let Some(old) = previous.filter(|old| {
        old["hysteria2"]["server_name"] == config["server_name"]
            && old["certificate"].is_string()
            && old["private_key"].is_string()
    }) {
        config["certificate"] = old["certificate"].clone();
        config["private_key"] = old["private_key"].clone();
    } else {
        let cert = rcgen::generate_simple_self_signed(vec![typed.server_name])?;
        config["certificate"] = cert.cert.pem().into();
        config["private_key"] = cert.key_pair.serialize_pem().into();
    }
    let certificate = config.as_object_mut().unwrap().remove("certificate").unwrap();
    let private_key = config.as_object_mut().unwrap().remove("private_key").unwrap();
    Ok(json!({"hysteria2":config,"certificate":certificate,"private_key":private_key}))
}
pub fn settings(config: &Value) -> anyhow::Result<Settings> {
    let mut public = config["hysteria2"].clone();
    if let Some(map) = public.as_object_mut() {
        map.remove("certificate");
        map.remove("private_key");
    }
    Ok(serde_json::from_value(public)?)
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn certificate_is_stable_until_sni_changes() {
        let request = json!({"hysteria2":{"server_name":"example.com"}});
        let first = persisted_config(&request, None).unwrap();
        let second = persisted_config(&request, Some(&first)).unwrap();
        assert_eq!(first, second);
        let changed =
            persisted_config(&json!({"hysteria2":{"server_name":"other.example.com"}}), Some(&first))
                .unwrap();
        assert_ne!(first["certificate"], changed["certificate"]);
        assert!(settings(&first).is_ok());
        assert!(!serde_json::to_value(settings(&first).unwrap())
            .unwrap()
            .to_string()
            .contains("PRIVATE KEY"));
    }
}

pub fn certificate_fingerprint(config: &Value) -> anyhow::Result<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let encoded: String = config["certificate"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing certificate"))?
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    Ok(hex::encode(Sha256::digest(der)))
}

#[cfg(test)]
mod request_tests {
    use super::*;
    #[test]
    fn hy2_request_requires_udp_port_and_empty_flow() {
        let mut request = json!({"name":"HY2","protocol":"hysteria2","address_type":"domain","address":"example.com","port":443,"enabled":true,"config":{"hysteria2":{"server_name":"example.com"}}});
        let parse =
            |value: Value| serde_json::from_value::<crate::proxy::ProxyRequest>(value).unwrap().into_draft();
        assert!(parse(request.clone()).is_ok());
        request["flow"] = json!("xtls-rprx-vision");
        assert!(parse(request.clone()).is_err());
        request["flow"] = json!("");
        request["port"] = json!(0);
        assert!(parse(request.clone()).is_err());
        request["port"] = json!(443);
        request["config"]["hysteria2"]["down_mbps"] = json!(0);
        assert!(parse(request).is_err());
    }
}
