use super::*;

/// Settings the panel may read. Secrets are deliberately excluded: the client can
/// set the GitHub secret but never read it back.
pub(super) const READABLE_SETTINGS: &[&str] = &[
    "site_name",
    "public_page",
    "github_client_id",
    "github_allowed_users",
    "retention_days",
    "theme",
    "github_proxy",
    "update_notice",
    "favicon",
    "touch_icon",
    "origin_cdn_only",
];

pub async fn sessions(_: Admin, State(app): State<Shared>, headers: HeaderMap) -> Response {
    crate::blocking::response(move || {
        let mine = current_session(&headers);
        match app.db.sessions() {
            Ok(rows) => Json(
                rows.into_iter()
                    .map(|(hash, expires_at)| {
                        json!({
                            "current": mine.as_deref() == Some(hash.as_str()),
                            "created_at": issued_at(expires_at),
                            "id": hash,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}

/// Deleting a row that no longer exists is not an error: two panels open on the
/// same list both achieve the requested sign-out.
pub async fn delete_session(_: Admin, State(app): State<Shared>, Path(id): Path<String>) -> Response {
    crate::blocking::response(move || match app.db.drop_session(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => fail(e),
    })
    .await
}

pub async fn settings(
    _: Admin,
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> Response {
    crate::blocking::response(move || {
        let mut out = serde_json::Map::new();
        // The address this request is taken for under the saved `origin_cdn_only`,
        // so the admin can see whether the switch recognizes them: behind a CDN the
        // proxy's own address means it does not.
        out.insert("your_address".into(), json!(client_ip(&app, &headers, peer.ip()).to_string()));
        for key in READABLE_SETTINGS {
            out.insert((*key).to_owned(), json!(app.db.get(key).unwrap_or_default()));
        }
        // The one readable key with a default that also rejects the empty string:
        // `setting_error` below refuses "" and `save_settings` writes nothing when any
        // key fails, so answering "" for a hub where this was never set would have
        // the entire settings form rejected, naming a field that was never edited.
        // `retention_days()` already holds the default `prune` and the data page read,
        // so it answers here as well.
        out.insert("retention_days".into(), json!(app.db.retention_days().to_string()));
        out.insert(
            "github_secret_set".into(),
            json!(app.db.get("github_client_secret").is_some_and(|v| !v.is_empty())),
        );
        // Read-only here. A window is opened and closed through its own route, so the
        // key is always one the hub generated, and `save_settings` continues to refuse
        // both stored names.
        out.insert("register_key".into(), json!(app.db.get("register_key").unwrap_or_default()));
        out.insert("register_left".into(), json!(register_left(&app)));
        crate::notify::settings(&app, &mut out);
        Json(Value::Object(out))
    })
    .await
}

/// Why one setting cannot be stored, or `None` when it can.
///
/// Separate from the write below because every key is validated before any is
/// written: changing the password drops every session, and a 400 raised
/// afterwards -- on a later key, in whatever order the map iterates -- carries no
/// Set-Cookie, signing the admin out of every device through a password change
/// the UI reported as rejected.
pub(super) fn setting_error(app: &App, key: &str, value: &Value) -> Option<String> {
    // Settings are stored as text. A caller sending the natural JSON type --
    // `{"public_page": false}`, `{"retention_days": 7}` -- is refused rather than
    // skipped, which would write nothing while the response reported success.
    let Some(value) = value.as_str() else { return Some(format!("设置 {key} 的值格式不对")) };
    match key {
        "theme" if !crate::frontend::selectable(app, value) => Some("主题没有安装".into()),
        // Housekeeping clamps whatever it reads, so an unparsable value would be
        // stored, echoed back, and silently mean the default indefinitely.
        "retention_days"
            if !value.parse::<i64>().is_ok_and(|d| (1..=db::MAX_RETENTION_DAYS).contains(&d)) =>
        {
            Some(format!("历史保留天数要在 1 到 {} 之间", db::MAX_RETENTION_DAYS))
        }
        // The hub fetches this URL itself, so it must be one: a scheme it cannot
        // speak turns every agent download into a 502 that says nothing about the
        // setting responsible.
        //
        // https only. What returns from this host is the agent binary, which
        // `install.sh` writes to /opt/monitor and starts on every node provisioned
        // here; over http:// anyone on the path between the hub and the mirror
        // chooses that binary, while the node still sees a valid TLS connection to
        // the hub.
        "github_proxy" if !(value.is_empty() || value.starts_with("https://")) => {
            Some("GitHub 代理必须以 https:// 开头：agent 程序经它下载，再安装到每个节点".into())
        }
        "favicon" | "touch_icon" if !value.is_empty() => {
            crate::frontend::site_icon(value).err().map(Into::into)
        }
        "admin_password" if value.len() < 12 => Some("密码至少 12 位".into()),
        "admin_password" => None,
        "origin_cdn_only" if !matches!(value, "" | "on" | "off") => {
            Some("源站只允许 CDN 访问：取值是 on 或 off".into())
        }
        k if k.starts_with("notify_") => crate::notify::setting_error(k, value),
        k if READABLE_SETTINGS.contains(&k) || k == "github_client_secret" => None,
        _ => Some(format!("没有这个设置项：{key}")),
    }
}

pub async fn save_settings(
    _: Admin,
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    crate::blocking::response(move || {
        let Some(map) = body.as_object() else { return bad("设置格式不对") };
        for (key, value) in map {
            if let Some(message) = setting_error(&app, key, value) {
                return bad(&message);
            }
        }
        // Set when the password changed: the change signs every session out, and the
        // caller receives a replacement rather than being logged out by it.
        let mut reissued = String::new();
        for (key, value) in map {
            let value = value.as_str().unwrap_or_default();
            if key == "admin_password" {
                match hash_password(value).and_then(|h| {
                    app.db.replace_password(&h)?;
                    issue_session(&app, &headers)
                }) {
                    Ok(cookie) => reissued = cookie,
                    Err(e) => return fail(e),
                }
                continue;
            }
            if let Err(e) = app.db.set(key, value) {
                return fail(e);
            }
        }
        with_cookies(Json(json!({"ok": true})), [reissued])
    })
    .await
}
