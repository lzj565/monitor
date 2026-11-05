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

pub async fn user_proxies(UserAuth(user): UserAuth, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match user_portal_items(&app, user) {
        Ok(items) => ([(header::CACHE_CONTROL, "no-store")], Json(json!({"items":items}))).into_response(),
        Err(e) => api_internal(e),
    })
    .await
}

/// One ordered collection drives the page, individual copies and Mihomo export.
pub(super) fn user_portal_items(app: &App, user: i64) -> anyhow::Result<Vec<Value>> {
    let now = Utc::now().timestamp();
    let prefs = app.db.all_entry_preferences(user)?;
    let mut entries: Vec<(String, Value, Option<crate::db::UserPortalProxy>)> = Vec::new();
    let defaults = app.db.internal_default_names()?;
    for mut proxy in app.db.active_user_portal_proxies(user, now)? {
        let key = db::internal_entry_key(proxy.id);
        let default = defaults.get(&proxy.id).cloned().unwrap_or_else(|| proxy.name.clone());
        proxy.name = default.clone();
        proxy.include_node_name = false;
        let online = {
            let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
            agents.contains_key(&proxy.node_id)
                && proxy.realm_target_node_id.is_none_or(|id| agents.contains_key(&id))
        };
        let mut item = serde_json::to_value(&proxy)?;
        item.as_object_mut().unwrap().remove("mieru_password");
        item["default_name"] = json!(default);
        item["online"] = json!(online);
        item["source"] = json!(if proxy.id > 0 { "proxy" } else { "realm" });
        entries.push((key, item, Some(proxy)));
    }
    for (id, config) in app.db.active_external_configs(user, now)? {
        let item = json!({"source":"external","default_name":config["name"],"name":config["name"],"protocol":config["type"],"address":config["server"],"address_type":"domain","port":config["port"],"node_name":"外部导入","node_group":"仅用于 Mihomo · 不采集统计","node_country":"","server_name":config.get("sni").or_else(||config.get("servername")).cloned().unwrap_or(json!("")),"online":null,"external_config":config});
        entries.push((format!("external:{id}"), item, None));
    }
    entries.sort_by_key(|(key, _, _)| prefs.get(key).and_then(|p| p.0).unwrap_or(i64::MAX));
    let mut names: std::collections::HashSet<String> =
        ["Proxy", "DIRECT", "direct"].into_iter().map(str::to_owned).collect();
    entries.into_iter().map(|(key,mut item,proxy)|{
        let pref=prefs.get(&key);
        let custom=pref.map(|p|p.2.as_str()).unwrap_or("");
        let default=item["default_name"].as_str().unwrap_or("代理");
        let mut name=if custom.is_empty(){default}else{custom}.to_owned();
        while !names.insert(name.clone()){name=format!("{name} [{key}]");}
        item["entry_key"]=json!(key);item["hidden"]=json!(pref.is_some_and(|p|p.1));
        item["custom_name"]=json!(custom);item["name"]=json!(name);
        if let Some(mut proxy)=proxy {
            proxy.name=name.clone();proxy.include_node_name=false;
            item["mihomo_yaml"]=json!(mihomo_proxy(user,&proxy));
            let host=if proxy.address.contains(':'){format!("[{}]",proxy.address)}else{proxy.address.clone()};
            if proxy.protocol=="mieru" {
                item["mieru_url"]=json!(format!("mierus://{}:{}@{}?profile={}&port={}&protocol=TCP",uri_component(&format!("user-{user}")),uri_component(proxy.mieru_password.as_deref().unwrap_or("")),host,uri_component(&name),proxy.port));
            } else {
                let flow=if proxy.flow.is_empty(){proxy::DEFAULT_FLOW}else{&proxy.flow};
                item["vless_url"]=json!(format!("vless://{}@{}:{}?encryption=none&security=reality&sni={}&fp=chrome&pbk={}&sid={}&type=tcp&flow={}#{}",proxy.uuid,host,proxy.port,uri_component(&proxy.server_name),proxy.public_key,proxy.short_id,uri_component(flow),uri_component(&name)));
            }
        } else {
            let mut config=item.as_object_mut().unwrap().remove("external_config").unwrap();config["name"]=json!(name);
            item["mihomo_yaml"]=json!(serde_yaml_ng::to_string(&vec![config])?.trim_end());
        }
        Ok(item)
    }).collect()
}

pub(super) fn combined_clash_subscription(items: &[Value]) -> String {
    let visible: Vec<_> = items.iter().filter(|p| p["hidden"] != true).collect();
    let mut out = if visible.is_empty() { "proxies: []\n".to_owned() } else { "proxies:\n".to_owned() };
    for item in &visible {
        for line in item["mihomo_yaml"].as_str().unwrap_or("").lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("proxy-groups:\n  - name: Proxy\n    type: select\n    proxies:\n");
    for item in visible {
        out.push_str(&format!("      - {}\n", yaml_string(item["name"].as_str().unwrap_or(""))));
    }
    out.push_str("      - DIRECT\nrules:\n  - MATCH,Proxy\n");
    out
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

pub(super) fn mihomo_proxy(user_id: i64, proxy: &crate::db::UserPortalProxy) -> String {
    let name = crate::db::displayed_proxy_name(&proxy.name, proxy.include_node_name, &proxy.node_country);
    if proxy.protocol == "mieru" {
        return format!(
            "- name: {}\n  type: mieru\n  server: {}\n  port: {}\n  transport: TCP\n  udp: true\n  username: {}\n  password: {}\n  multiplexing: MULTIPLEXING_OFF\n  handshake-mode: HANDSHAKE_NO_WAIT",
            yaml_string(&name), yaml_ip_or_string(&proxy.address), proxy.port,
            yaml_string(&format!("user-{user_id}")), yaml_string(proxy.mieru_password.as_deref().expect("authorized mieru proxy has credentials")),
        );
    }
    let flow = if proxy.flow.is_empty() { proxy::DEFAULT_FLOW } else { &proxy.flow };
    format!(
        "- name: {}\n  type: vless\n  server: {}\n  port: {}\n  uuid: {}\n  network: tcp\n  tls: true\n  udp: true\n  servername: {}\n  client-fingerprint: chrome\n  reality-opts:\n    public-key: {}\n    short-id: {}\n  flow: {}",
        yaml_string(&name), yaml_ip_or_string(&proxy.address), proxy.port, proxy.uuid,
        yaml_string(&proxy.server_name), yaml_string(&proxy.public_key), yaml_string(&proxy.short_id), yaml_string(flow),
    )
}

#[cfg(test)]
pub(super) fn clash_subscription(user_id: i64, proxies: &[crate::db::UserPortalProxy]) -> String {
    let mut out = String::from(if proxies.is_empty() { "proxies: []\n" } else { "proxies:\n" });
    let labels: Vec<_> = proxies
        .iter()
        .map(|p| crate::db::displayed_proxy_name(&p.name, p.include_node_name, &p.node_country))
        .collect();
    for proxy in proxies {
        for line in mihomo_proxy(user_id, proxy).lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
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
        let (content_type, body) = match format.as_str() {
            "clash" => {
                let items = match user_portal_items(&app, user_id) {
                    Ok(items) => items,
                    Err(e) => return api_internal(e),
                };
                ("application/yaml; charset=utf-8", combined_clash_subscription(&items))
            }

            "sing-box" => {
                let (uuid, mut proxies) =
                    match app.db.active_user_subscription(user_id, Utc::now().timestamp()) {
                        Ok(Some(data)) => data,
                        Ok(None) => {
                            return api_error(StatusCode::FORBIDDEN, "ACCOUNT_INACTIVE", "账号当前不可用")
                        }
                        Err(error) => return api_internal(error),
                    };
                let names = match user_portal_items(&app, user_id) {
                    Ok(items) => items,
                    Err(e) => return api_internal(e),
                };
                for proxy in &mut proxies {
                    if let Some(item) =
                        names.iter().find(|item| item["entry_key"] == db::internal_entry_key(proxy.id))
                    {
                        proxy.name = item["name"].as_str().unwrap_or(&proxy.name).to_owned();
                        proxy.include_node_name = false;
                    }
                }
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

#[derive(Deserialize)]
pub struct EntryPreference {
    pub(super) hidden: Option<bool>,
    pub(super) custom_name: Option<String>,
}
pub async fn save_user_entry_preferences(
    UserAuth(user): UserAuth,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(pref): Json<EntryPreference>,
) -> Response {
    crate::blocking::response(move || {
        match app.db.save_entry_preferences(user, id, pref.hidden, pref.custom_name) {
            Ok(()) => Json(json!({"ok":true})).into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}
