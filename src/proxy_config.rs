//! ProxyNode 到 sing-box 配置的纯转换逻辑。

use std::collections::BTreeSet;
use std::fmt;
use std::net::IpAddr;

use serde_json::{json, Value};

use crate::db::{ProxyNode, ProxyUser};

/// Monitor 管理 inbound 使用的 tag 前缀；用户配置应避开此命名空间。
pub const MANAGED_INBOUND_TAG_PREFIX: &str = "monitor-proxy-node-";

/// 生成配置时返回不含任何节点字段的错误，避免密钥进入日志或错误响应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyConfigError {
    InvalidNodeId,
    InvalidCurrentConfig,
    InvalidProxyNodeId,
    DuplicateProxyNodeId,
    InvalidProxyUserId,
    DuplicateProxyUserId,
    InvalidPort,
    DuplicateListenPort,
    UnsupportedProtocol,
    InvalidUuid,
    InvalidShortId,
    InvalidRealityDest,
    InvalidRealityPrivateKey,
    InvalidRealityServerName,
    InvalidListenAddress,
}

impl fmt::Display for ProxyConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidNodeId => "服务器 ID 无效",
            Self::InvalidCurrentConfig => {
                "现有 sing-box 配置必须是 JSON 对象，且 inbounds（如存在）必须是数组"
            }
            Self::InvalidProxyNodeId => "代理节点 ID 无效",
            Self::DuplicateProxyNodeId => "代理节点 ID 重复",
            Self::InvalidProxyUserId => "代理用户 ID 无效",
            Self::DuplicateProxyUserId => "代理用户 ID 重复",
            Self::InvalidPort => "代理节点监听端口无效",
            Self::DuplicateListenPort => "同一服务器上的代理节点监听端口重复",
            Self::UnsupportedProtocol => "代理节点协议不受支持",
            Self::InvalidUuid => "VLESS UUID 格式无效",
            Self::InvalidShortId => "Reality Short ID 格式无效",
            Self::InvalidRealityDest => "Reality Dest 必须是有效的 host:port 或 [IPv6]:port",
            Self::InvalidRealityPrivateKey => "Reality 私钥不能为空",
            Self::InvalidRealityServerName => "Reality Server Name 不能为空",
            Self::InvalidListenAddress => "sing-box inbound 监听地址必须是 IPv4 或 IPv6 地址",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ProxyConfigError {}

/// 保留现有 sing-box 配置中的用户内容，并以目标服务器的 ProxyNode 重建托管 inbound。
///
/// 该函数不访问数据库、文件或 Agent；调用方负责提供当前配置和节点记录。
pub fn generate_singbox_config(
    node_id: i64,
    current_config: Value,
    nodes: &[ProxyNode],
) -> Result<Value, ProxyConfigError> {
    generate_singbox_config_excluding_users(node_id, current_config, nodes, &[], &[])
}

/// 导入和删除可以额外声明本次部署应移除的原始 inbound tag。
pub fn generate_singbox_config_excluding(
    node_id: i64,
    current_config: Value,
    nodes: &[ProxyNode],
    additionally_excluded_tags: &[String],
) -> Result<Value, ProxyConfigError> {
    generate_singbox_config_excluding_users(node_id, current_config, nodes, &[], additionally_excluded_tags)
}

/// 使用当前服务器上被授权且启用的 ProxyUser 重建托管 inbound。
pub fn generate_singbox_config_excluding_users(
    node_id: i64,
    mut current_config: Value,
    nodes: &[ProxyNode],
    users: &[ProxyUser],
    additionally_excluded_tags: &[String],
) -> Result<Value, ProxyConfigError> {
    if node_id <= 0 {
        return Err(ProxyConfigError::InvalidNodeId);
    }

    let mut excluded_source_tags = nodes
        .iter()
        .filter(|node| node.node_id == node_id)
        .filter_map(|node| node.source_inbound_tag.clone())
        .collect::<BTreeSet<_>>();
    excluded_source_tags.extend(additionally_excluded_tags.iter().cloned());

    let mut managed_nodes =
        nodes.iter().filter(|node| node.node_id == node_id && node.enabled).collect::<Vec<_>>();
    managed_nodes.sort_unstable_by_key(|node| node.id);

    let mut seen_ids = BTreeSet::new();
    let mut seen_ports = BTreeSet::new();
    let mut generated = Vec::with_capacity(managed_nodes.len());
    for node in managed_nodes {
        if node.id <= 0 {
            return Err(ProxyConfigError::InvalidProxyNodeId);
        }
        if !seen_ids.insert(node.id) {
            return Err(ProxyConfigError::DuplicateProxyNodeId);
        }
        if node.listen_port == 0 {
            return Err(ProxyConfigError::InvalidPort);
        }
        if !seen_ports.insert(node.listen_port) {
            return Err(ProxyConfigError::DuplicateListenPort);
        }
        generated.push(generate_inbound(node, users)?);
    }

    let Some(config) = current_config.as_object_mut() else {
        return Err(ProxyConfigError::InvalidCurrentConfig);
    };
    let inbounds = config.entry("inbounds").or_insert_with(|| json!([]));
    let Value::Array(inbounds) = inbounds else {
        return Err(ProxyConfigError::InvalidCurrentConfig);
    };

    let mut merged = Vec::with_capacity(inbounds.len() + generated.len());
    for inbound in inbounds.drain(..) {
        let monitor_managed = inbound
            .get("tag")
            .and_then(Value::as_str)
            .is_some_and(|tag| tag.starts_with(MANAGED_INBOUND_TAG_PREFIX));
        let source_managed =
            inbound.get("tag").and_then(Value::as_str).is_some_and(|tag| excluded_source_tags.contains(tag));
        if !monitor_managed && !source_managed {
            merged.push(inbound);
        }
    }
    merged.extend(generated);
    *inbounds = merged;

    Ok(current_config)
}

fn generate_inbound(node: &ProxyNode, users: &[ProxyUser]) -> Result<Value, ProxyConfigError> {
    if node.protocol != "vless_reality" {
        return Err(ProxyConfigError::UnsupportedProtocol);
    }
    if !is_reality_short_id(&node.reality_short_id) {
        return Err(ProxyConfigError::InvalidShortId);
    }
    if node.reality_private_key.trim().is_empty() {
        return Err(ProxyConfigError::InvalidRealityPrivateKey);
    }
    if node.reality_server_name.trim().is_empty() {
        return Err(ProxyConfigError::InvalidRealityServerName);
    }
    if node.listen_address.parse::<IpAddr>().is_err() {
        return Err(ProxyConfigError::InvalidListenAddress);
    }

    let (server, server_port) = parse_reality_dest(&node.reality_dest)?;
    let mut inbound_users = Vec::new();
    let mut seen_user_ids = BTreeSet::new();
    let mut seen_user_uuids = BTreeSet::new();
    let mut assigned_users = users
        .iter()
        .filter(|user| user.enabled && user.proxy_node_ids.contains(&node.id))
        .collect::<Vec<_>>();
    assigned_users.sort_unstable_by_key(|user| user.id);
    for user in assigned_users {
        if user.id <= 0 {
            return Err(ProxyConfigError::InvalidProxyUserId);
        }
        if !seen_user_ids.insert(user.id) {
            return Err(ProxyConfigError::DuplicateProxyUserId);
        }
        if !is_uuid(&user.uuid) {
            return Err(ProxyConfigError::InvalidUuid);
        }
        if seen_user_uuids.insert(user.uuid.to_ascii_lowercase()) {
            inbound_users.push(json!({
                "name": format!("monitor-user-{}", user.id),
                "uuid": user.uuid,
                "flow": "xtls-rprx-vision"
            }));
        }
    }
    Ok(json!({
        "type": "vless",
        "tag": format!("{MANAGED_INBOUND_TAG_PREFIX}{}", node.id),
        "listen": node.listen_address,
        "listen_port": node.listen_port,
        "users": inbound_users,
        "tls": {
            "enabled": true,
            "server_name": node.reality_server_name,
            "reality": {
                "enabled": true,
                "handshake": {
                    "server": server,
                    "server_port": server_port
                },
                "private_key": node.reality_private_key.trim(),
                "short_id": [node.reality_short_id.trim()]
            }
        }
    }))
}

pub(crate) fn is_uuid(value: &str) -> bool {
    let value = value.trim();
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

pub(crate) fn is_reality_short_id(value: &str) -> bool {
    // sing-box 用十六进制文本表示最多 8 字节的 Reality Short ID。
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 16
        && value.len().is_multiple_of(2)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_reality_dest(value: &str) -> Result<(String, u16), ProxyConfigError> {
    let value = value.trim();
    let (host, port) = if let Some(bracketed) = value.strip_prefix('[') {
        let (host, port) = bracketed.split_once("]:").ok_or(ProxyConfigError::InvalidRealityDest)?;
        let address = host.parse::<IpAddr>().map_err(|_| ProxyConfigError::InvalidRealityDest)?;
        if !matches!(address, IpAddr::V6(_)) {
            return Err(ProxyConfigError::InvalidRealityDest);
        }
        (host, port)
    } else {
        let (host, port) = value.split_once(':').ok_or(ProxyConfigError::InvalidRealityDest)?;
        if host.contains(':') {
            return Err(ProxyConfigError::InvalidRealityDest);
        }
        validate_host(host)?;
        (host, port)
    };

    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProxyConfigError::InvalidRealityDest);
    }
    let port = port.parse::<u16>().map_err(|_| ProxyConfigError::InvalidRealityDest)?;
    if port == 0 {
        return Err(ProxyConfigError::InvalidRealityDest);
    }
    Ok((host.to_owned(), port))
}

/// 校验部署前的 Reality SNI 和握手目标，避免先创建无法生成配置的代理节点。
pub fn validate_reality_settings(server_name: &str, dest: &str) -> Result<(), ProxyConfigError> {
    validate_host(server_name.trim()).map_err(|_| ProxyConfigError::InvalidRealityServerName)?;
    parse_reality_dest(dest)?;
    Ok(())
}

fn validate_host(host: &str) -> Result<(), ProxyConfigError> {
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return Err(ProxyConfigError::InvalidRealityDest);
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }

    let domain = host.strip_suffix('.').unwrap_or(host);
    if domain.is_empty()
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(ProxyConfigError::InvalidRealityDest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_node(id: i64, node_id: i64, enabled: bool) -> ProxyNode {
        ProxyNode {
            id,
            node_id,
            name: format!("node-{id}"),
            enabled,
            protocol: "vless_reality".into(),
            address_mode: "custom".into(),
            custom_address: Some("proxy.example.com".into()),
            listen_port: 10_000 + id.rem_euclid(20_000) as u16,
            uuid: "bf000d23-0752-40b4-affe-68f7707a9661".into(),
            reality_private_key: "private-secret".into(),
            reality_public_key: "public-secret".into(),
            reality_short_id: "0123456789abcdef".into(),
            reality_server_name: "www.apple.com".into(),
            reality_dest: "www.apple.com:443".into(),
            listen_address: crate::db::DEFAULT_PROXY_LISTEN_ADDRESS.into(),
            source_inbound_tag: None,
            deploy_status: "not_deployed".into(),
            last_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn proxy_user(id: i64, uuid: &str, enabled: bool, proxy_node_ids: &[i64]) -> ProxyUser {
        ProxyUser {
            id,
            name: format!("user-{id}"),
            uuid: uuid.into(),
            enabled,
            is_system: false,
            created_at: 1,
            note: String::new(),
            updated_at: 1,
            proxy_node_ids: proxy_node_ids.to_vec(),
        }
    }

    fn empty_config() -> Value {
        json!({"inbounds": []})
    }

    #[test]
    fn generates_one_vless_reality_inbound_with_only_proxy_user_identity() {
        let mut node = proxy_node(12, 1, true);
        node.uuid = "legacy-node-uuid-is-not-an-identity".into();
        let generated = generate_singbox_config_excluding_users(
            1,
            empty_config(),
            &[node],
            &[proxy_user(7, "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e", true, &[12])],
            &[],
        )
        .unwrap();
        let inbound = &generated["inbounds"][0];
        assert_eq!(
            inbound,
            &json!({
                "type": "vless",
                "tag": "monitor-proxy-node-12",
                "listen": "::",
                "listen_port": 10012,
                "users": [{
                    "name": "monitor-user-7",
                    "uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
                    "flow": "xtls-rprx-vision"
                }],
                "tls": {
                    "enabled": true,
                    "server_name": "www.apple.com",
                    "reality": {
                        "enabled": true,
                        "handshake": {"server": "www.apple.com", "server_port": 443},
                        "private_key": "private-secret",
                        "short_id": ["0123456789abcdef"]
                    }
                }
            })
        );
        assert!(inbound.get("public_key").is_none());
        assert!(inbound["tls"]["reality"].get("public_key").is_none());
        assert_eq!(inbound["listen"], "::", "custom client address must not become a bind address");
        assert!(!inbound["users"].to_string().contains("legacy-node-uuid-is-not-an-identity"));
    }

    #[test]
    fn keeps_managed_inbound_with_no_users_when_none_are_enabled_and_authorized() {
        let generated = generate_singbox_config_excluding_users(
            1,
            empty_config(),
            &[proxy_node(12, 1, true)],
            &[
                proxy_user(2, "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e", false, &[12]),
                proxy_user(3, "aa8c947a-1dbd-4905-bcb1-71728c58effb", true, &[18]),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(generated["inbounds"][0]["users"], json!([]));
    }

    #[test]
    fn system_admin_uses_the_same_enabled_and_node_authorization_filters() {
        let node_one = proxy_node(12, 1, true);
        let node_two = proxy_node(18, 1, true);
        let mut admin = proxy_user(1, "0d46b1aa-49ea-4889-998f-c3a1cd52942f", true, &[12]);
        admin.name = "admin".into();
        admin.is_system = true;
        let generated = generate_singbox_config_excluding_users(
            1,
            empty_config(),
            &[node_one.clone(), node_two.clone()],
            &[admin.clone()],
            &[],
        )
        .unwrap();
        assert_eq!(generated["inbounds"][0]["users"][0]["uuid"], admin.uuid);
        assert_eq!(generated["inbounds"][1]["users"], json!([]));
        assert!(!generated["inbounds"].to_string().contains(&node_one.uuid));
        assert!(!generated["inbounds"].to_string().contains(&node_two.uuid));

        admin.enabled = false;
        let disabled =
            generate_singbox_config_excluding_users(1, empty_config(), &[node_one, node_two], &[admin], &[])
                .unwrap();
        assert_eq!(disabled["inbounds"][0]["users"], json!([]));
        assert_eq!(disabled["inbounds"][1]["users"], json!([]));
    }

    #[test]
    fn generates_only_enabled_users_authorized_for_each_node() {
        let node = proxy_node(12, 1, true);
        let node_uuid = node.uuid.clone();
        let second_node = proxy_node(18, 1, true);
        let mut first_user = proxy_user(2, "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e", true, &[12, 18]);
        first_user.name = "Sensitive Display Name".into();
        let mut admin_user = proxy_user(7, "0d46b1aa-49ea-4889-998f-c3a1cd52942f", false, &[12]);
        admin_user.name = "admin".into();
        admin_user.is_system = true;
        let generated = generate_singbox_config_excluding_users(
            1,
            empty_config(),
            &[node, second_node],
            &[
                first_user,
                proxy_user(3, "aa8c947a-1dbd-4905-bcb1-71728c58effb", true, &[12]),
                proxy_user(4, "e2b4b1d8-0af6-40f8-910b-cabfb913a7bc", false, &[12]),
                proxy_user(5, "e91f28f5-5837-4b92-a281-224d155f01f6", true, &[18]),
                proxy_user(6, "f3a9c7de-bc1a-4239-8c56-05e7a0984e41", true, &[999]),
                admin_user,
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            generated["inbounds"][0]["users"],
            json!([
                {
                    "name": "monitor-user-2",
                    "uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
                    "flow": "xtls-rprx-vision"
                },
                {
                    "name": "monitor-user-3",
                    "uuid": "aa8c947a-1dbd-4905-bcb1-71728c58effb",
                    "flow": "xtls-rprx-vision"
                }
            ])
        );
        let credential_users = generated["inbounds"][0]["users"].to_string();
        assert!(credential_users.contains("monitor-user-2"));
        assert!(!credential_users.contains("Sensitive Display Name"));
        assert!(!credential_users.contains(&node_uuid));
        assert!(!credential_users.contains("e2b4b1d8-0af6-40f8-910b-cabfb913a7bc"));
        assert!(!credential_users.contains("f3a9c7de-bc1a-4239-8c56-05e7a0984e41"));
        assert!(!credential_users.contains("0d46b1aa-49ea-4889-998f-c3a1cd52942f"));
        assert_eq!(
            generated["inbounds"][1]["users"][0]["uuid"], "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
            "同一用户授权多个代理节点时使用同一个 UUID"
        );
        assert_eq!(generated["inbounds"][1]["users"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn omits_disabled_nodes_and_nodes_from_other_servers() {
        let mut foreign = proxy_node(3, 2, true);
        foreign.protocol = "unsupported".into();
        let generated =
            generate_singbox_config(1, empty_config(), &[proxy_node(1, 1, false), foreign]).unwrap();
        assert_eq!(generated["inbounds"], json!([]));
    }

    #[test]
    fn managed_nodes_are_sorted_by_id_regardless_of_input_order() {
        let generated = generate_singbox_config(
            1,
            empty_config(),
            &[proxy_node(9, 1, true), proxy_node(2, 1, true), proxy_node(5, 1, true)],
        )
        .unwrap();
        let tags = generated["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|inbound| inbound["tag"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(tags, ["monitor-proxy-node-2", "monitor-proxy-node-5", "monitor-proxy-node-9"]);
    }

    #[test]
    fn preserves_user_inbounds_and_replaces_old_managed_inbounds() {
        let config = json!({
            "inbounds": [
                {"type": "socks", "tag": "user-managed-inbound"},
                {"type": "vless", "tag": "monitor-proxy-node-1", "old": true},
                {"type": "vless", "tag": "monitor-proxy-node-999", "old": true}
            ]
        });
        let generated =
            generate_singbox_config(1, config, &[proxy_node(1, 1, true), proxy_node(2, 1, true)]).unwrap();
        let inbounds = generated["inbounds"].as_array().unwrap();
        assert_eq!(inbounds[0]["tag"], "user-managed-inbound");
        let tags = inbounds.iter().map(|inbound| inbound["tag"].as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(tags, ["user-managed-inbound", "monitor-proxy-node-1", "monitor-proxy-node-2"]);
        assert!(inbounds.iter().all(|inbound| inbound.get("old").is_none()));
    }

    #[test]
    fn preserves_all_other_top_level_configuration() {
        let config = json!({
            "log": {"level": "warn", "timestamp": true},
            "dns": {"servers": [{"tag": "dns"}]},
            "outbounds": [{"type": "direct", "tag": "direct"}],
            "route": {"rules": [{"domain": ["example.com"]}]},
            "experimental": {"cache_file": {"enabled": true}},
            "endpoints": [{"type": "wireguard", "tag": "wg"}],
            "inbounds": [{"type": "socks", "tag": "user"}]
        });
        let generated = generate_singbox_config(1, config.clone(), &[proxy_node(1, 1, true)]).unwrap();
        for key in ["log", "dns", "outbounds", "route", "experimental", "endpoints"] {
            assert_eq!(generated[key], config[key], "top-level field {key} changed");
        }
        assert_eq!(generated["inbounds"][0]["tag"], "user");
    }

    #[test]
    fn replaces_the_imported_source_inbound_and_preserves_its_listener() {
        let mut node = proxy_node(12, 1, true);
        node.source_inbound_tag = Some("legacy-reality".into());
        node.listen_address = "0.0.0.0".into();
        let current = json!({
            "inbounds": [
                {"type": "vless", "tag": "legacy-reality", "listen_port": 33333},
                {"type": "socks", "tag": "manual-socks"}
            ]
        });

        let generated = generate_singbox_config(1, current, &[node.clone()]).unwrap();
        let inbounds = generated["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0]["tag"], "manual-socks");
        assert_eq!(inbounds[1]["tag"], "monitor-proxy-node-12");
        assert_eq!(inbounds[1]["listen"], "0.0.0.0");

        let redeployed = generate_singbox_config(1, generated.clone(), &[node]).unwrap();
        assert_eq!(redeployed, generated, "后续部署保持幂等");
    }

    #[test]
    fn parses_domain_ipv4_and_bracketed_ipv6_reality_destinations() {
        assert_eq!(parse_reality_dest("www.apple.com:443").unwrap(), ("www.apple.com".into(), 443));
        assert_eq!(parse_reality_dest("1.2.3.4:8443").unwrap(), ("1.2.3.4".into(), 8443));
        assert_eq!(parse_reality_dest("[2001:db8::1]:443").unwrap(), ("2001:db8::1".into(), 443));
    }

    #[test]
    fn rejects_malformed_config_dest_uuid_short_id_protocol_and_port() {
        let mut config = empty_config();
        config["inbounds"] = json!({"not": "an array"});
        assert_eq!(generate_singbox_config(1, config, &[]), Err(ProxyConfigError::InvalidCurrentConfig));
        assert_eq!(generate_singbox_config(1, json!([]), &[]), Err(ProxyConfigError::InvalidCurrentConfig));

        let mut node = proxy_node(1, 1, true);
        node.reality_dest = "2001:db8::1:443".into();
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[node.clone()]),
            Err(ProxyConfigError::InvalidRealityDest)
        );
        node = proxy_node(1, 1, true);
        node.uuid = "legacy-node-uuid-is-not-an-identity".into();
        let generated = generate_singbox_config_excluding_users(
            1,
            empty_config(),
            &[node.clone()],
            &[proxy_user(2, "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e", true, &[1])],
            &[],
        )
        .unwrap();
        assert_eq!(generated["inbounds"][0]["users"][0]["uuid"], "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e");
        assert_eq!(
            generate_singbox_config_excluding_users(
                1,
                empty_config(),
                &[node.clone()],
                &[proxy_user(2, "not-a-uuid", true, &[1])],
                &[],
            ),
            Err(ProxyConfigError::InvalidUuid)
        );
        node = proxy_node(1, 1, true);
        node.reality_short_id = "xyz".into();
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[node.clone()]),
            Err(ProxyConfigError::InvalidShortId)
        );
        node = proxy_node(1, 1, true);
        node.protocol = "xray".into();
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[node.clone()]),
            Err(ProxyConfigError::UnsupportedProtocol)
        );
        node = proxy_node(1, 1, true);
        node.listen_port = 0;
        assert_eq!(generate_singbox_config(1, empty_config(), &[node]), Err(ProxyConfigError::InvalidPort));

        let mut node = proxy_node(1, 1, true);
        node.listen_address = "eth0".into();
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[node]),
            Err(ProxyConfigError::InvalidListenAddress)
        );
    }

    #[test]
    fn rejects_duplicate_managed_ids_and_ports() {
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[proxy_node(1, 1, true), proxy_node(1, 1, true)]),
            Err(ProxyConfigError::DuplicateProxyNodeId)
        );
        let mut second = proxy_node(2, 1, true);
        second.listen_port = 10_001;
        assert_eq!(
            generate_singbox_config(1, empty_config(), &[proxy_node(1, 1, true), second]),
            Err(ProxyConfigError::DuplicateListenPort)
        );
    }

    #[test]
    fn errors_never_include_reality_private_key() {
        let mut node = proxy_node(1, 1, true);
        node.reality_private_key = "do-not-log-this-private-key".into();
        node.reality_dest = "invalid-dest".into();
        let error = generate_singbox_config(1, empty_config(), &[node]).unwrap_err();
        assert!(!error.to_string().contains("do-not-log-this-private-key"));
        assert!(!format!("{error:?}").contains("do-not-log-this-private-key"));
    }

    #[test]
    fn invalid_dest_never_panics_for_unusual_input() {
        for value in [
            "",
            ":443",
            "example.com:",
            "example.com:0",
            "example.com:65536",
            "[::1]443",
            "[127.0.0.1]:443",
            "bad..host:443",
        ] {
            assert_eq!(parse_reality_dest(value), Err(ProxyConfigError::InvalidRealityDest), "{value}");
        }
        assert!(parse_reality_dest("[::1]:443").is_ok());
    }
}
