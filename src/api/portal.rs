use super::*;

pub async fn me(State(app): State<Shared>, headers: HeaderMap) -> Response {
    crate::blocking::response(move || {
        let principal = match crate::auth::principal(&app, &headers) {
            Some(crate::db::SessionPrincipal::Admin) => "admin",
            Some(crate::db::SessionPrincipal::User(_)) => "user",
            None => "anonymous",
        };
        Json(json!({
            "authed": authed(&app, &headers),
            "principal": principal,
            "github": app.db.get("github_client_id").is_some_and(|v| !v.is_empty()),
            "site_name": app.db.get("site_name").unwrap_or_else(|| "Monitor".into()),
            "public_page": app.public_page(),
            // How far back a chart may reach, so a theme offers only windows the hub
            // answers in full; `metrics` narrows a wider one without saying so.
            "history_days": app.db.retention_days(),
            // Whether this browser may provision is not answered here: a GET carries
            // no `Origin`, so the panel applies `provisioning_allowed`'s rule itself.
            //
            // The hub's own public URL when one was given, which is what belongs in an
            // install command and in the OAuth callback -- not whichever address this
            // browser used, which behind a proxy may be a loopback port. Empty by
            // default, in which case the browser's address is the only one available
            // and the panel falls back to its own origin.
            "site": app.site,
        }))
    })
    .await
}

pub async fn user_me(UserAuth(user_id): UserAuth, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.user(user_id) {
        Ok(Some(user)) => {
            match app.db.user_traffic_overview(user_id, user.traffic_reset_day, Utc::now().timestamp()) {
                Ok(traffic) => Json(json!({
                    "id": user.id, "username": user.username, "enabled": user.enabled,
                    "expires_at": user.expires_at, "traffic_limit": user.traffic_limit,
                    "traffic_reset_day": user.traffic_reset_day, "traffic": traffic,
                }))
                .into_response(),
                Err(error) => api_internal(error),
            }
        }
        Ok(None) => api_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "登录已失效，请重新登录"),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn user_proxies(UserAuth(user_id): UserAuth, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || {
        let now = Utc::now().timestamp();
        match app.db.active_user_portal_proxies(user_id, now) {
            Ok(mut proxies) => {
                let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
                for proxy in &mut proxies {
                    proxy.online = agents.contains_key(&proxy.node_id) && proxy.realm_target_node_id.is_none_or(|id|agents.contains_key(&id));
                }
                let items = proxies.into_iter().map(|proxy| {
                    let label = crate::db::displayed_proxy_name(
                        &proxy.name,
                        proxy.include_node_name,
                        &proxy.node_country,
                    );
                    let flow = if proxy.flow.is_empty() { proxy::DEFAULT_FLOW } else { &proxy.flow };
                    let host = if proxy.address.parse::<std::net::Ipv6Addr>().is_ok() {
                        format!("[{}]", proxy.address)
                    } else {
                        proxy.address.clone()
                    };
                    let vless_url = format!(
                        "vless://{}@{}:{}?encryption=none&security=reality&sni={}&fp=chrome&pbk={}&sid={}&type=tcp&flow={}#{}",
                        proxy.uuid, host, proxy.port,
                        uri_component(&proxy.server_name), proxy.public_key, proxy.short_id,
                        uri_component(flow), uri_component(&label),
                    );
                    let mihomo_yaml = format!(
                        "- name: {}\n  type: vless\n  server: {}\n  port: {}\n  uuid: {}\n  network: tcp\n  tls: true\n  udp: true\n  servername: {}\n  client-fingerprint: chrome\n  reality-opts:\n    public-key: {}\n    short-id: {}\n  flow: {}",
                        yaml_string(&label), yaml_ip_or_string(&proxy.address), proxy.port,
                        proxy.uuid, yaml_string(&proxy.server_name), yaml_string(&proxy.public_key),
                        yaml_string(&proxy.short_id), yaml_string(flow),
                    );
                    let mut item = serde_json::to_value(proxy).expect("portal proxy serializes");
                    item["vless_url"] = json!(vless_url);
                    item["mihomo_yaml"] = json!(mihomo_yaml);
                    item
                }).collect::<Vec<_>>();
                Json(json!({"items": items})).into_response()
            }
            Err(error) => api_internal(error),
        }
    }).await
}

pub(super) fn uri_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub async fn reorder_user_proxies(
    UserAuth(user_id): UserAuth,
    State(app): State<Shared>,
    Json(order): Json<Order>,
) -> Response {
    crate::blocking::response(move || {
        match app.db.reorder_user_proxies(user_id, &order.ids, Utc::now().timestamp()) {
            Ok(()) => Json(json!({"ok": true})).into_response(),
            Err(error) => fail(error),
        }
    })
    .await
}

pub async fn user_subscription_links(UserAuth(user_id): UserAuth, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.user_subscription_token(user_id) {
        Ok(Some(token)) => Json(json!({
            "clash": format!("/api/subscriptions/{token}/clash"),
            "sing_box": format!("/api/subscriptions/{token}/sing-box"),
        }))
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn rotate_user_subscription_token(
    UserAuth(user_id): UserAuth,
    State(app): State<Shared>,
) -> Response {
    crate::blocking::response(move || match app.db.rotate_user_subscription_token(user_id) {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

pub(super) fn subscription_label(proxy: &crate::db::UserSubscriptionProxy) -> String {
    crate::db::displayed_proxy_name(&proxy.name, proxy.include_node_name, &proxy.node_country)
}

pub(super) fn yaml_string(value: &str) -> String {
    // Block scalars may contain Unicode, spaces and digits. Quote values whose
    // YAML implicit type or syntax could change the original string.
    let lower = value.to_ascii_lowercase();
    let unsigned = lower.trim_start_matches(['+', '-']);
    let numeric = value.replace('_', "").parse::<f64>().is_ok()
        || unsigned.starts_with("0x")
        || unsigned.starts_with("0o")
        || unsigned.starts_with("0b")
        || (unsigned.chars().next().is_some_and(|c| c.is_ascii_digit())
            && value.chars().all(|c| {
                c.is_ascii_digit() || matches!(c, '-' | ':' | '+' | '.' | 'T' | 'Z' | 't' | 'z' | ' ')
            }));
    let plain = !value.is_empty()
        && value.trim() == value
        && !value
            .starts_with([',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`'])
        && !(value.starts_with(['-', '?', ':'])
            && (value.len() == 1 || value.chars().nth(1).is_some_and(char::is_whitespace)))
        && !matches!(value, "---" | "...")
        && !value.chars().any(|c| c.is_control())
        && !value.contains(": ")
        && !value.ends_with(':')
        && !value.contains(" #")
        && !numeric
        && !matches!(
            lower.as_str(),
            "null"
                | "~"
                | "true"
                | "false"
                | "y"
                | "n"
                | "yes"
                | "no"
                | "on"
                | "off"
                | ".inf"
                | "+.inf"
                | "-.inf"
                | ".nan"
        );
    if plain {
        value.to_owned()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
    }
}

pub(super) fn yaml_ip_or_string(value: &str) -> String {
    if value.parse::<std::net::IpAddr>().is_ok() {
        value.to_owned()
    } else {
        yaml_string(value)
    }
}

pub(super) fn clash_subscription(uuid: &str, proxies: &[crate::db::UserSubscriptionProxy]) -> String {
    let mut out = String::from("proxies:\n");
    let labels: Vec<_> = proxies.iter().map(subscription_label).collect();
    for (proxy, name) in proxies.iter().zip(&labels) {
        out.push_str(&format!(
            "  - name: {}\n    type: vless\n    server: {}\n    port: {}\n    uuid: {}\n    network: tcp\n    tls: true\n    udp: true\n    servername: {}\n    client-fingerprint: chrome\n    reality-opts:\n      public-key: {}\n      short-id: {}\n",
            yaml_string(name), yaml_ip_or_string(&proxy.address), proxy.port, uuid,
            yaml_string(&proxy.server_name), yaml_string(&proxy.public_key), yaml_string(&proxy.short_id),
        ));
        let flow = if proxy.flow.is_empty() { proxy::DEFAULT_FLOW } else { &proxy.flow };
        out.push_str(&format!("    flow: {}\n", yaml_string(flow)));
    }
    out.push_str("proxy-groups:\n  - name: Proxy\n    type: select\n    proxies:\n");
    for name in &labels {
        out.push_str(&format!("      - {}\n", yaml_string(name)));
    }
    out.push_str("      - DIRECT\nrules:\n  - MATCH,Proxy\n");
    out
}

pub(super) fn sing_box_subscription(uuid: &str, proxies: &[crate::db::UserSubscriptionProxy]) -> Value {
    let mut outbounds: Vec<Value> = proxies
        .iter()
        .map(|proxy| {
            let name = subscription_label(proxy);
            let mut outbound = json!({
                "type": "vless",
                "tag": name,
                "server": proxy.address,
                "server_port": proxy.port,
                "uuid": uuid,
                "network": "tcp",
                "tls": {
                    "enabled": true,
                    "server_name": proxy.server_name,
                    "utls": {"enabled": true, "fingerprint": "chrome"},
                    "reality": {"enabled": true, "public_key": proxy.public_key, "short_id": proxy.short_id}
                }
            });
            outbound["flow"] = Value::String(if proxy.flow.is_empty() {
                proxy::DEFAULT_FLOW.to_owned()
            } else {
                proxy.flow.clone()
            });
            outbound
        })
        .collect();
    let tags: Vec<String> = proxies.iter().map(subscription_label).collect();
    if !tags.is_empty() {
        outbounds.push(json!({"type":"selector","tag":"Proxy","outbounds":tags,"default":tags[0]}));
    }
    outbounds.push(json!({"type":"direct","tag":"direct"}));
    let final_tag = if tags.is_empty() { "direct" } else { "Proxy" };
    json!({
        "$schema": "https://sing-box.sagernet.org/schema.json",
        "outbounds": outbounds,
        "route": {"final": final_tag}
    })
}

pub async fn subscription(
    State(app): State<Shared>,
    Path((token, format)): Path<(String, String)>,
) -> Response {
    crate::blocking::response(move || {
        let user_id = match app.db.active_user_id_for_subscription_token(&token, Utc::now().timestamp()) {
            Ok(Some(id)) => id,
            Ok(None) => {
                return api_error(StatusCode::NOT_FOUND, "SUBSCRIPTION_NOT_FOUND", "订阅链接无效或已过期")
            }
            Err(error) => return api_internal(error),
        };
        let (uuid, proxies) = match app.db.active_user_subscription(user_id, Utc::now().timestamp()) {
            Ok(Some(data)) => data,
            Ok(None) => return api_error(StatusCode::FORBIDDEN, "ACCOUNT_INACTIVE", "账号当前不可用"),
            Err(error) => return api_internal(error),
        };
        let (content_type, body) = match format.as_str() {
            "clash" => ("application/yaml; charset=utf-8", clash_subscription(&uuid, &proxies)),
            "sing-box" => {
                ("application/json; charset=utf-8", sing_box_subscription(&uuid, &proxies).to_string())
            }
            _ => return api_error(StatusCode::NOT_FOUND, "SUBSCRIPTION_FORMAT_NOT_FOUND", "订阅格式不存在"),
        };
        let mut response = ([(header::CONTENT_TYPE, content_type)], body).into_response();
        response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().expect("static header"));
        response
    })
    .await
}
