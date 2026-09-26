//! 为已部署的 ProxyNode 生成客户端可导入的 VLESS Reality URI。

use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use url::{Host, Url};

use crate::api::{self, Admin};
use crate::db::{Node, ProxyNode};
use crate::{Shared, Shown};

#[derive(Serialize)]
struct ProxyNodeShare {
    node_id: i64,
    name: String,
    address: String,
    uri: String,
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn share_allowed(proxy_node: &ProxyNode) -> Result<(), &'static str> {
    if !proxy_node.enabled {
        return Err("代理节点已停用，启用并部署后才能生成分享链接");
    }
    if proxy_node.deploy_status != "deployed" {
        return Err(match proxy_node.deploy_status.as_str() {
            "deploying" => "代理节点正在部署，部署成功后才能生成分享链接",
            "failed" => "代理节点部署失败，重新部署成功后才能生成分享链接",
            _ => "代理节点尚未成功部署，部署成功后才能生成分享链接",
        });
    }
    if proxy_node.protocol != "vless_reality" {
        return Err("当前代理节点协议不支持分享链接");
    }
    Ok(())
}

fn selected_address(proxy_node: &ProxyNode, server: &Node) -> anyhow::Result<String> {
    let address = match proxy_node.address_mode.as_str() {
        "custom" => proxy_node.custom_address.as_deref().unwrap_or_default().trim().to_owned(),
        "ipv4" | "ipv6" => {
            let ipv6 = proxy_node.address_mode == "ipv6";
            api::addresses(&server.ip, (&server.ipv4, &server.ipv6), (&server.ipv4_pin, &server.ipv6_pin))
                .into_iter()
                .find_map(|(address, _)| {
                    address.parse::<IpAddr>().ok().filter(|ip| ip.is_ipv6() == ipv6).map(|ip| ip.to_string())
                })
                .ok_or_else(|| {
                    anyhow::Error::msg(Shown(if ipv6 {
                        "所属服务器没有可用的 IPv6 地址".into()
                    } else {
                        "所属服务器没有可用的 IPv4 地址".into()
                    }))
                })?
        }
        _ => refuse!("代理节点地址模式无效"),
    };
    if address.is_empty() {
        refuse!("代理节点连接地址为空");
    }
    Ok(address)
}

fn uri_host(address: &str) -> anyhow::Result<String> {
    let address = address.trim();
    let unbracketed = address.strip_prefix('[').and_then(|value| value.strip_suffix(']')).unwrap_or(address);
    if let Ok(ip) = unbracketed.parse::<IpAddr>() {
        return Ok(match ip {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        });
    }
    if address.chars().any(|c| matches!(c, ':' | '/' | '?' | '#' | '@' | '\\' | '%')) {
        refuse!("连接地址必须是 IPv4、IPv6 或域名，不能包含端口或路径");
    }
    let parsed = Url::parse(&format!("http://{address}/"))
        .map_err(|_| anyhow::Error::msg(Shown("连接地址不是有效的 IPv4、IPv6 或域名".into())))?;
    match parsed.host() {
        Some(Host::Domain(domain)) if !domain.is_empty() => Ok(domain.to_owned()),
        Some(Host::Ipv4(ip)) => Ok(ip.to_string()),
        Some(Host::Ipv6(ip)) => Ok(format!("[{ip}]")),
        _ => refuse!("连接地址不是有效的 IPv4、IPv6 或域名"),
    }
}

fn valid_uuid(uuid: &str) -> bool {
    let bytes = uuid.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn build_share(proxy_node: &ProxyNode, server: &Node) -> anyhow::Result<ProxyNodeShare> {
    share_allowed(proxy_node).map_err(|message| anyhow::Error::msg(Shown(message.into())))?;
    if proxy_node.listen_port == 0 {
        refuse!("代理节点监听端口无效");
    }
    if !valid_uuid(&proxy_node.uuid) {
        refuse!("代理节点 UUID 无效");
    }
    for (value, label) in [
        (proxy_node.reality_public_key.as_str(), "Reality 公钥"),
        (proxy_node.reality_short_id.as_str(), "Reality Short ID"),
        (proxy_node.reality_server_name.as_str(), "Reality SNI"),
    ] {
        if value.trim().is_empty() {
            refuse!("代理节点缺少{label}");
        }
    }

    let host = uri_host(&selected_address(proxy_node, server)?)?;
    let address = format!("{host}:{}", proxy_node.listen_port);
    let query = [
        ("encryption", "none"),
        ("security", "reality"),
        ("sni", proxy_node.reality_server_name.trim()),
        ("fp", "chrome"),
        ("pbk", proxy_node.reality_public_key.trim()),
        ("sid", proxy_node.reality_short_id.trim()),
        ("type", "tcp"),
        ("flow", "xtls-rprx-vision"),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", encode_uri_component(value)))
    .collect::<Vec<_>>()
    .join("&");
    let uri =
        format!("vless://{}@{address}?{query}#{}", proxy_node.uuid, encode_uri_component(&proxy_node.name));

    Ok(ProxyNodeShare { node_id: proxy_node.id, name: proxy_node.name.clone(), address, uri })
}

pub async fn share_proxy_node(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    let proxy_node = match app.db.proxy_node(id) {
        Ok(Some(proxy_node)) => proxy_node,
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理节点不存在")),
        Err(error) => return no_store(api::fail(error)),
    };
    let server = match app.db.node(proxy_node.node_id) {
        Ok(Some(server)) => server,
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "所属服务器不存在")),
        Err(error) => return no_store(api::fail(error)),
    };
    if let Err(message) = share_allowed(&proxy_node) {
        return no_store(api::answer(StatusCode::CONFLICT, message));
    }
    match build_share(&proxy_node, &server) {
        Ok(share) => no_store(Json(share).into_response()),
        Err(error) => no_store(api::fail(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn proxy_node() -> ProxyNode {
        ProxyNode {
            id: 7,
            node_id: 3,
            name: "HK Reality".into(),
            enabled: true,
            protocol: "vless_reality".into(),
            address_mode: "ipv4".into(),
            custom_address: None,
            listen_port: 24443,
            uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            reality_private_key: "private-key-must-never-leave-the-server".into(),
            reality_public_key: "public-key".into(),
            reality_short_id: "1234abcd".into(),
            reality_server_name: "www.apple.com".into(),
            reality_dest: "www.apple.com:443".into(),
            deploy_status: "deployed".into(),
            last_error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn server() -> Node {
        Node {
            id: 3,
            name: "server".into(),
            ipv4: "198.51.100.20".into(),
            ipv6: "2001:db8::20".into(),
            ..Default::default()
        }
    }

    fn shown_error(result: anyhow::Result<ProxyNodeShare>) -> String {
        result
            .err()
            .expect("分享信息不完整时应返回可操作错误")
            .downcast_ref::<Shown>()
            .map(|error| error.0.clone())
            .unwrap_or_default()
    }

    #[test]
    fn builds_ipv4_vless_reality_uri() {
        let share = build_share(&proxy_node(), &server()).unwrap();
        assert_eq!(share.address, "198.51.100.20:24443");
        let uri = Url::parse(&share.uri).unwrap();
        assert_eq!(uri.scheme(), "vless");
        assert_eq!(uri.username(), "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(uri.host_str(), Some("198.51.100.20"));
        assert_eq!(uri.port(), Some(24443));
        let query: std::collections::HashMap<_, _> = uri.query_pairs().into_owned().collect();
        assert_eq!(query.get("encryption").map(String::as_str), Some("none"));
        assert_eq!(query.get("security").map(String::as_str), Some("reality"));
        assert_eq!(query.get("sni").map(String::as_str), Some("www.apple.com"));
        assert_eq!(query.get("fp").map(String::as_str), Some("chrome"));
        assert_eq!(query.get("pbk").map(String::as_str), Some("public-key"));
        assert_eq!(query.get("sid").map(String::as_str), Some("1234abcd"));
        assert_eq!(query.get("type").map(String::as_str), Some("tcp"));
        assert_eq!(query.get("flow").map(String::as_str), Some("xtls-rprx-vision"));
        assert_eq!(uri.fragment(), Some("HK%20Reality"));
        assert!(!share.uri.contains("private-key-must-never-leave-the-server"));
    }

    #[test]
    fn brackets_ipv6_authority() {
        let mut proxy_node = proxy_node();
        proxy_node.address_mode = "ipv6".into();
        let share = build_share(&proxy_node, &server()).unwrap();
        assert_eq!(share.address, "[2001:db8::20]:24443");
        let uri = Url::parse(&share.uri).unwrap();
        assert_eq!(uri.host_str(), Some("[2001:db8::20]"));
    }

    #[test]
    fn custom_domain_and_unicode_fragment_are_encoded() {
        let mut proxy_node = proxy_node();
        proxy_node.address_mode = "custom".into();
        proxy_node.custom_address = Some("edge.example.net".into());
        proxy_node.name = "香港 [主节点] #1 🚀".into();
        proxy_node.reality_server_name = "cdn.example.net&x=1".into();
        let share = build_share(&proxy_node, &server()).unwrap();
        assert_eq!(share.address, "edge.example.net:24443");
        assert!(share.uri.contains("sni=cdn.example.net%26x%3D1"));
        assert!(
            share
                .uri
                .contains("#%E9%A6%99%E6%B8%AF%20%5B%E4%B8%BB%E8%8A%82%E7%82%B9%5D%20%231%20%F0%9F%9A%80"),
            "{}",
            share.uri
        );
        assert_eq!(
            Url::parse(&share.uri).unwrap().fragment(),
            Some("%E9%A6%99%E6%B8%AF%20%5B%E4%B8%BB%E8%8A%82%E7%82%B9%5D%20%231%20%F0%9F%9A%80")
        );
    }

    #[test]
    fn rejects_missing_addresses_credentials_and_unavailable_state() {
        let mut proxy_node = proxy_node();
        let mut server = server();
        server.ipv4.clear();
        server.ipv6.clear();
        assert!(shown_error(build_share(&proxy_node, &server)).contains("IPv4"));
        proxy_node.address_mode = "ipv6".into();
        assert!(shown_error(build_share(&proxy_node, &server)).contains("IPv6"));

        proxy_node.address_mode = "custom".into();
        proxy_node.custom_address = Some("edge.example.net".into());
        proxy_node.reality_public_key.clear();
        assert!(shown_error(build_share(&proxy_node, &server)).contains("Reality 公钥"));

        proxy_node.reality_public_key = "public-key".into();
        proxy_node.enabled = false;
        assert!(shown_error(build_share(&proxy_node, &server)).contains("已停用"));
        proxy_node.enabled = true;
        proxy_node.deploy_status = "failed".into();
        assert!(shown_error(build_share(&proxy_node, &server)).contains("部署失败"));
    }

    #[test]
    fn serialized_node_and_share_never_contain_private_key() {
        let proxy_node = proxy_node();
        let serialized = serde_json::to_string(&proxy_node).unwrap();
        assert!(!serialized.contains("reality_private_key"));
        assert!(!serialized.contains("private-key-must-never-leave-the-server"));
        let share = build_share(&proxy_node, &server()).unwrap();
        let serialized_share = serde_json::to_string(&share).unwrap();
        assert!(!serialized_share.contains("private-key-must-never-leave-the-server"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized_share).unwrap()["node_id"],
            json!(7)
        );
    }

    #[test]
    fn rejects_custom_authority_injection_and_brackets_ipv6_custom_address() {
        assert!(uri_host("example.com:443").is_err());
        assert!(uri_host("example.com/path").is_err());
        assert_eq!(uri_host("[2001:db8::1]").unwrap(), "[2001:db8::1]");
    }
}
