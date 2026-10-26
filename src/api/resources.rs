use super::*;

pub(super) const MAX_COMMAND_CONFIG: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub(super) struct QueuedNodeSync {
    pub(super) core: &'static str,
    pub(super) node_id: i64,
    pub(super) command_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct NeedsNodeSync {
    pub(super) core: &'static str,
    pub(super) node_id: i64,
    pub(super) reason: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct SyncReport {
    pub(super) queued: Vec<QueuedNodeSync>,
    pub(super) pending: Vec<NeedsNodeSync>,
    pub(super) needs_sync: Vec<NeedsNodeSync>,
}

#[derive(Debug)]
pub(super) enum EnqueueFailure {
    TooLarge,
    Offline,
    QueueFull,
    Unavailable,
}

pub(super) fn with_sync<T: Serialize>(value: &T, sync: &SyncReport) -> Value {
    let mut value = serde_json::to_value(value).unwrap_or_else(|_| json!({}));
    if let Value::Object(object) = &mut value {
        object.insert("sync".into(), serde_json::to_value(sync).unwrap_or_else(|_| json!({})));
    }
    value
}

pub(super) fn empty_params() -> Value {
    json!({})
}

#[derive(Deserialize)]
pub struct CommandRequest {
    pub(super) method: String,
    #[serde(default = "empty_params")]
    pub(super) params: Value,
}

/// Enqueues one allowlisted sing-box action on the Agent's current WebSocket.
/// The Admin extractor keeps this control path private to signed-in operators.
pub async fn create_command(
    _: Admin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: Result<Json<CommandRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let Json(body) = match body {
            Ok(body) => body,
            Err(error) => return error.into_response(),
        };
        if body.method.starts_with("mieru.") {
            if body.method == "mieru.config.apply" {
                return bad("请通过 mieru 代理页面应用配置");
            }
            if !app
                .agents
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&node_id)
                .is_some_and(|a| a.mieru_capable)
            {
                return bad("节点离线或 Agent 需要升级");
            }
        }
        if matches!(body.method.as_str(), "realm.config.apply" | "realm.restart" | "realm.traffic.configure")
        {
            return bad("请通过 Realm 页面应用配置和重启");
        }
        if body.method.starts_with("realm.") {
            if !app
                .agents
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&node_id)
                .is_some_and(|a| a.realm_capable)
            {
                return api_error(StatusCode::CONFLICT, "REALM_AGENT_REQUIRED", "节点离线或 Agent 需要升级");
            }
            if body.method == "realm.config.check"
                && body
                    .params
                    .get("config")
                    .is_none_or(|v| !v.is_object() || v.to_string().len() > MAX_COMMAND_CONFIG)
            {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "REALM_CONFIG_INVALID",
                    "Realm 配置必须是 1 MiB 以内的 JSON 对象",
                );
            }
        }
        if !command::supports(&body.method) {
            return bad("不支持的 Agent 命令");
        }
        if !body.params.is_object() {
            return bad("params 必须是 JSON 对象");
        }
        if matches!(body.method.as_str(), "singbox.config.check" | "singbox.config.apply") {
            let Some(content) = body.params.get("content").and_then(Value::as_str) else {
                return bad("params.content 必须是字符串");
            };
            if content.len() > MAX_COMMAND_CONFIG {
                return answer(StatusCode::PAYLOAD_TOO_LARGE, "sing-box 配置不能超过 1 MiB");
            }
        }
        match app.db.node(node_id) {
            Ok(Some(_)) => {}
            Ok(None) => return no_such_node(),
            Err(error) => return fail(error),
        }

        // Keep the read guard until the reservation and try_send are complete. A
        // reconnect takes the write lock, then retires commands from the session it
        // replaced; this closes the window where a request could be registered too
        // late for that retirement.
        let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
        let Some(agent) = agents.get(&node_id) else {
            return answer(StatusCode::SERVICE_UNAVAILABLE, "Agent 当前离线");
        };
        let session = agent.session;
        let id = match app.commands.start(node_id, session, &body.method) {
            Ok(id) => id,
            Err(StartError::Full) => {
                return answer(StatusCode::SERVICE_UNAVAILABLE, "命令队列已满，请稍后重试");
            }
        };
        let message = json!({
            "type": "command",
            "id": id,
            "action": body.method,
            "params": body.params,
        })
        .to_string();
        match agent.tx.try_send(message) {
            Ok(()) => (StatusCode::ACCEPTED, Json(json!({"command_id": id}))).into_response(),
            Err(_) => {
                app.commands.cancel(&id, node_id, session);
                answer(StatusCode::SERVICE_UNAVAILABLE, "Agent 命令队列已满或连接已关闭")
            }
        }
    })
    .await
}

/// Reads a command's in-memory state. Command IDs are scoped to the node in
/// both the registry lookup and the URL, so they cannot be used across nodes.
pub async fn command_status(
    _: Admin,
    State(app): State<Shared>,
    Path((node_id, command_id)): Path<(i64, String)>,
) -> Response {
    crate::blocking::response(move || {
        match app.db.node(node_id) {
            Ok(Some(_)) => {}
            Ok(None) => return no_such_node(),
            Err(error) => return fail(error),
        }
        match app.commands.get(&command_id, node_id) {
            Some(command) => Json(command).into_response(),
            None => answer(StatusCode::NOT_FOUND, "命令不存在或已过期"),
        }
    })
    .await
}

// ---- V1 Proxy, User and sing-box Desired State API ----

pub(super) const PROXY_TRAFFIC_ACTIVE_WINDOW: i64 = 60;

#[derive(Deserialize, Default)]
pub struct ProxyTrafficRowsQuery {
    pub(super) q: Option<String>,
    pub(super) node_id: Option<i64>,
    pub(super) sort: Option<String>,
    pub(super) order: Option<String>,
    pub(super) page: Option<i64>,
    pub(super) page_size: Option<i64>,
}

pub(super) fn proxy_traffic_page_args(
    query: &ProxyTrafficRowsQuery,
) -> (Option<&str>, Option<i64>, &str, bool, i64, i64) {
    let search = query.q.as_deref().map(str::trim).filter(|value| !value.is_empty());
    let sort = match query.sort.as_deref() {
        Some("uplink") => "uplink",
        Some("downlink") => "downlink",
        _ => "total",
    };
    let descending = query.order.as_deref() != Some("asc");
    let page = query.page.unwrap_or(1).clamp(1, i64::MAX / 100);
    let page_size = query.page_size.unwrap_or(25).clamp(1, 100);
    (search, query.node_id.filter(|id| *id > 0), sort, descending, page, page_size)
}

pub async fn proxy_traffic_overview(_: ApiAdmin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || {
        let active_since = Utc::now().timestamp().saturating_sub(PROXY_TRAFFIC_ACTIVE_WINDOW);
        match app.db.proxy_traffic_overview(active_since) {
            Ok(summary) => Json(summary).into_response(),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn proxy_traffic_user_rows(
    _: ApiAdmin,
    State(app): State<Shared>,
    Query(query): Query<ProxyTrafficRowsQuery>,
) -> Response {
    crate::blocking::response(move || {
        let (search, node_id, sort, descending, page, page_size) = proxy_traffic_page_args(&query);
        match app.db.proxy_traffic_user_rows(search, node_id, sort, descending, page, page_size) {
            Ok(result) => Json(result).into_response(),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn proxy_traffic_node_rows(
    _: ApiAdmin,
    State(app): State<Shared>,
    Query(query): Query<ProxyTrafficRowsQuery>,
) -> Response {
    crate::blocking::response(move || {
        let (search, node_id, sort, descending, page, page_size) = proxy_traffic_page_args(&query);
        match app.db.proxy_traffic_node_rows(search, node_id, sort, descending, page, page_size) {
            Ok(result) => Json(result).into_response(),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn proxy_user_traffic(_: ApiAdmin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.proxy_user_traffic() {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn proxy_user_traffic_by_node(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(user_id): Path<i64>,
) -> Response {
    crate::blocking::response(move || {
        if let Err(response) = api_user(&app, user_id) {
            return response;
        }
        match app.db.proxy_user_traffic_by_node(user_id) {
            Ok(items) => Json(json!({"items":items})).into_response(),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn proxy_node_traffic(_: ApiAdmin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.proxy_node_traffic() {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn reset_proxy_user_traffic(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(user_id): Path<i64>,
) -> Response {
    crate::blocking::response(move || {
        if let Err(response) = api_user(&app, user_id) {
            return response;
        }
        match app.db.reset_proxy_user_traffic(user_id, Utc::now().timestamp()) {
            Ok(true) => StatusCode::NO_CONTENT.into_response(),
            Ok(false) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn reset_proxy_node_traffic(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
) -> Response {
    crate::blocking::response(move || {
        if let Err(response) = api_node_exists(&app, node_id) {
            return response;
        }
        match app.db.reset_proxy_node_traffic(node_id, Utc::now().timestamp()) {
            Ok(true) => StatusCode::NO_CONTENT.into_response(),
            Ok(false) => api_error(StatusCode::NOT_FOUND, "NODE_NOT_FOUND", "node does not exist"),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn list_all_proxies(_: ApiAdmin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.all_proxies() {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn list_proxies(_: ApiAdmin, State(app): State<Shared>, Path(node_id): Path<i64>) -> Response {
    crate::blocking::response(move || {
        if let Err(response) = api_node_exists(&app, node_id) {
            return response;
        }
        match app.db.proxies_for_node(node_id) {
            Ok(items) => Json(json!({"items":items})).into_response(),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn create_proxy(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: Result<Json<ProxyRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let draft = match request.into_draft() {
            Ok(draft) => draft,
            Err(error) => return api_input_error(error),
        };
        match app.db.create_proxy(node_id, &draft) {
            Ok(Ok(id)) => {
                let sync = sync_generated_nodes(&app, vec![node_id]);
                match app.db.proxy(id) {
                    Ok(Some(proxy)) => (StatusCode::CREATED, Json(with_sync(&proxy, &sync))).into_response(),
                    Ok(None) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", INTERNAL),
                    Err(error) => api_internal(error),
                }
            }
            Ok(Err(issue)) => map_proxy_issue(issue),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn get_proxy(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.proxy(id) {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn update_proxy(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<ProxyRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let node_id = request.node_id;
        let draft = match request.into_draft() {
            Ok(draft) => draft,
            Err(error) => return api_input_error(error),
        };
        let previous_node_id = match app.db.proxy(id) {
            Ok(proxy) => proxy.map(|proxy| proxy.node_id),
            Err(error) => return api_internal(error),
        };
        match app.db.update_proxy(id, node_id, &draft) {
            Ok(Ok(())) => match app.db.proxy(id) {
                Ok(Some(proxy)) => {
                    let sync = sync_generated_nodes(
                        &app,
                        previous_node_id.into_iter().chain(std::iter::once(proxy.node_id)).collect(),
                    );
                    Json(with_sync(&proxy, &sync)).into_response()
                }
                Ok(None) => api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist"),
                Err(error) => api_internal(error),
            },
            Ok(Err(issue)) => map_proxy_issue(issue),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn delete_proxy(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || {
        let node_id = match app.db.proxy(id) {
            Ok(proxy) => proxy.map(|proxy| proxy.node_id),
            Err(error) => return api_internal(error),
        };
        match app.db.realm_refs(None, Some(id)) {
            Ok(refs) if !refs.is_empty() => {
                return api_error(
                    StatusCode::CONFLICT,
                    "REALM_REFERENCED",
                    format!(
                        "代理仍被 Realm 规则引用：{}",
                        refs.iter().filter_map(|r| r["name"].as_str()).collect::<Vec<_>>().join("、")
                    ),
                )
            }
            Err(e) => return fail(e),
            _ => {}
        }
        match app.db.delete_proxy(id) {
            Ok(true) => {
                let sync = sync_generated_nodes(&app, node_id.into_iter().collect());
                Json(json!({"sync":sync})).into_response()
            }
            Ok(false) => api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist"),
            Err(error) => api_internal(error),
        }
    })
    .await
}

#[derive(Deserialize)]
pub struct UserQuery {
    pub(super) enabled: Option<bool>,
    pub(super) q: Option<String>,
}

pub async fn list_users(_: ApiAdmin, State(app): State<Shared>, Query(query): Query<UserQuery>) -> Response {
    crate::blocking::response(move || match app.db.users(query.enabled, query.q.as_deref()) {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn create_user(
    _: ApiAdmin,
    State(app): State<Shared>,
    body: Result<Json<UserRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let draft = match request.into_draft(true) {
            Ok(draft) => draft,
            Err(error) => return api_input_error(error),
        };
        match app.db.create_user(&draft) {
            Ok(Ok(id)) => match app.db.user(id) {
                Ok(Some(user)) => (StatusCode::CREATED, Json(user)).into_response(),
                Ok(None) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", INTERNAL),
                Err(error) => api_internal(error),
            },
            Ok(Err(issue)) => map_user_issue(issue),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn get_user(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.user(id) {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn reset_user_uuid(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.reset_user_uuid(id) {
        Ok(Some((uuid, nodes))) => {
            let sync = sync_generated_nodes(&app, nodes);
            let queued = sync.queued.iter().map(|item| item.node_id).collect::<Vec<_>>();
            let offline = sync.needs_sync.iter().map(|item| item.node_id).collect::<Vec<_>>();
            Json(json!({"uuid":uuid,"queued_node_ids":queued,"needs_sync_node_ids":offline,"sync":sync}))
                .into_response()
        }
        Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

pub async fn update_user(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<UserRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let uuid = match request.normalized_uuid() {
            Ok(uuid) => uuid,
            Err(error) => return api_input_error(error),
        };
        let draft = match request.into_draft(false) {
            Ok(draft) => draft,
            Err(error) => return api_input_error(error),
        };
        let old_user = match app.db.user(id) {
            Ok(user) => user,
            Err(error) => return api_internal(error),
        };
        // Always synchronize explicit UUID writes, including concurrent saves that
        // may have changed the UUID since the snapshot above was read.
        let affects_config = uuid.is_some()
            || old_user.as_ref().is_some_and(|user| {
                user.username != draft.username
                    || user.enabled != draft.enabled
                    || user.expires_at != draft.expires_at
            });
        let result = match uuid.as_deref() {
            Some(uuid) => app.db.update_user_with_uuid(id, &draft, Some(uuid)),
            None => app.db.update_user(id, &draft),
        };
        match result {
            Ok(Ok(())) => match app.db.user(id) {
                Ok(Some(user)) => {
                    let sync = if affects_config {
                        match app.db.nodes_for_user(id) {
                            Ok(nodes) => sync_generated_nodes(&app, nodes),
                            Err(error) => return api_internal(error),
                        }
                    } else {
                        SyncReport::default()
                    };
                    Json(with_sync(&user, &sync)).into_response()
                }
                Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
                Err(error) => api_internal(error),
            },
            Ok(Err(issue)) => map_user_issue(issue),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn delete_user(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || {
        let nodes = match app.db.nodes_for_user(id) {
            Ok(nodes) => nodes,
            Err(error) => return api_internal(error),
        };
        match app.db.delete_user(id) {
            Ok(UserDeleteResult::Deleted) => {
                let sync = sync_generated_nodes(&app, nodes);
                Json(json!({"sync":sync})).into_response()
            }
            Ok(UserDeleteResult::NotFound) => {
                api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist")
            }
            Ok(UserDeleteResult::Protected) => api_error(
                StatusCode::CONFLICT,
                "SYSTEM_USER_PROTECTED",
                "the default admin user cannot be deleted",
            ),
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn get_user_authorizations(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(user_id): Path<i64>,
) -> Response {
    crate::blocking::response(move || match app.db.authorizations_for_user(user_id) {
        Ok(Some(items)) => Json(json!({"items":items})).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist"),
        Err(error) => api_internal(error),
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAuthorizationsRequest {
    pub items: Vec<ReplaceAuthorizationItem>,
    pub auto_authorize_new_proxies: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAuthorizationItem {
    pub proxy_id: i64,
    pub enabled: bool,
}

/// Atomically replaces a user's complete proxy authorization set. The nodes
/// from both the old and new sets are synchronized once each after commit.
pub async fn replace_user_authorizations(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(user_id): Path<i64>,
    body: Result<Json<ReplaceAuthorizationsRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        if let Err(response) = api_user(&app, user_id) {
            return response;
        }
        let mut seen = std::collections::HashSet::new();
        for item in &request.items {
            if !seen.insert(item.proxy_id) {
                return api_error(StatusCode::BAD_REQUEST, "DUPLICATE_PROXY", "proxy_id must be unique");
            }
            let proxy = match api_proxy(&app, item.proxy_id) {
                Ok(proxy) => proxy,
                Err(response) => return response,
            };
            if !proxy::supported_protocol(&proxy) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "INVALID_PROXY_PROTOCOL",
                    "Only VLESS and mieru authorization are supported",
                );
            }
        }
        match app.db.replace_authorizations_with_policy(
            user_id,
            &request.items.iter().map(|item| (item.proxy_id, item.enabled)).collect::<Vec<_>>(),
            request.auto_authorize_new_proxies,
        ) {
            Ok(Ok(node_ids)) => {
                let sync = sync_generated_nodes(&app, node_ids);
                Json(json!({"sync":sync})).into_response()
            }
            Ok(Err(AuthorizationWriteIssue::UserNotFound)) => {
                api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist")
            }
            Ok(Err(AuthorizationWriteIssue::ProxyNotFound)) => {
                api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist")
            }
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn put_user_proxy(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path((user_id, proxy_id)): Path<(i64, i64)>,
    body: Result<Json<AuthorizationRequest>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let request = match api_body(body) {
            Ok(request) => request,
            Err(response) => return response,
        };
        let draft: AuthorizationDraft = match request.into_draft() {
            Ok(draft) => draft,
            Err(error) => return api_input_error(error),
        };
        if let Err(response) = api_user(&app, user_id) {
            return response;
        }
        let proxy = match api_proxy(&app, proxy_id) {
            Ok(proxy) => proxy,
            Err(response) => return response,
        };
        if !proxy::supported_protocol(&proxy) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "INVALID_PROXY_PROTOCOL",
                "Only VLESS and mieru authorization are supported",
            );
        }
        match app.db.put_authorization(user_id, proxy_id, &draft) {
            Ok(Ok(access)) => {
                let sync = sync_generated_nodes(&app, vec![proxy.node_id]);
                Json(json!({
                    "user_id": access.user_id,
                    "proxy_id": access.proxy_id,
                    "enabled": access.enabled,
                    "auth": access.auth,
                    "sync": sync,
                }))
                .into_response()
            }
            Ok(Err(AuthorizationWriteIssue::UserNotFound)) => {
                api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist")
            }
            Ok(Err(AuthorizationWriteIssue::ProxyNotFound)) => {
                api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist")
            }
            Err(error) => api_internal(error),
        }
    })
    .await
}

pub async fn delete_user_proxy(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path((user_id, proxy_id)): Path<(i64, i64)>,
) -> Response {
    crate::blocking::response(move || {
        if let Err(response) = api_user(&app, user_id) {
            return response;
        }
        let proxy = match api_proxy(&app, proxy_id) {
            Ok(proxy) => proxy,
            Err(response) => return response,
        };
        match app.db.delete_authorization(user_id, proxy_id) {
            Ok(true) => {
                let sync = sync_generated_nodes(&app, vec![proxy.node_id]);
                Json(json!({"sync":sync})).into_response()
            }
            Ok(false) => {
                api_error(StatusCode::NOT_FOUND, "ACCESS_NOT_FOUND", "user has no access to this proxy")
            }
            Err(error) => api_internal(error),
        }
    })
    .await
}

#[allow(clippy::result_large_err)]
pub(super) fn generated_node_config(app: &Shared, node_id: i64) -> Result<Value, Response> {
    api_node_exists(app, node_id)?;
    proxy::generate_config(&app.db, node_id, Utc::now().timestamp()).map_err(|error| {
        if error.to_string().contains("generated config exceeds 1 MiB") {
            api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "SINGBOX_CONFIG_TOO_LARGE",
                "generated config exceeds 1 MiB",
            )
        } else {
            api_internal(error)
        }
    })
}

pub(super) fn sync_generated_nodes(app: &Shared, node_ids: Vec<i64>) -> SyncReport {
    let mut report = SyncReport::default();
    let node_ids = node_ids.into_iter().collect::<std::collections::BTreeSet<_>>();
    for node_id in node_ids {
        match app.db.mieru_desired(node_id) {
            Ok(Some(_)) => {
                report.pending.push(NeedsNodeSync { core: "mieru", node_id, reason: "reconciling" })
            }
            Err(_) => {
                report.needs_sync.push(NeedsNodeSync { core: "mieru", node_id, reason: "config_error" })
            }
            _ => {}
        }
        let has_vless =
            app.db.proxies_for_node(node_id).is_ok_and(|p| p.iter().any(|p| p.protocol == "vless"));
        if has_vless {
            if let Err(e) = app.db.set(&format!("singbox_managed_node_{node_id}"), "1") {
                tracing::warn!("cannot record sing-box ownership: {e:#}");
            }
        }
        if !has_vless
            && app.db.get(&format!("singbox_managed_node_{node_id}")).is_none()
            && app.db.mieru_managed(node_id).unwrap_or(false)
        {
            continue;
        }
        match generated_node_config(app, node_id) {
            Ok(config) => {
                match enqueue_generated_config_id(app, node_id, "singbox.config.apply", config.to_string()) {
                    Ok(command_id) => {
                        report.queued.push(QueuedNodeSync { core: "singbox", node_id, command_id })
                    }
                    Err(error) => report.needs_sync.push(NeedsNodeSync {
                        core: "singbox",
                        node_id,
                        reason: error.as_str(),
                    }),
                }
            }
            Err(_) => {
                report.needs_sync.push(NeedsNodeSync { core: "singbox", node_id, reason: "config_error" })
            }
        }
    }
    report
}

pub(crate) fn sync_node_config_on_connect(app: &Shared, node_id: i64) -> bool {
    if app
        .db
        .proxies_for_node(node_id)
        .is_ok_and(|p| !p.is_empty() && p.iter().all(|p| p.protocol == "mieru"))
        && app.db.get(&format!("singbox_managed_node_{node_id}")).is_none()
    {
        return true;
    }
    let Ok(config) = generated_node_config(app, node_id) else { return false };
    enqueue_generated_config(app, node_id, "singbox.config.apply", config.to_string()).status().is_success()
}

pub(crate) fn sync_node_config_for_expired_user(app: &App, node_id: i64) -> Result<String, &'static str> {
    let config =
        proxy::generate_config(&app.db, node_id, Utc::now().timestamp()).map_err(|_| "config_error")?;
    enqueue_generated_config_id(app, node_id, "singbox.config.apply", config.to_string())
        .map_err(|error| error.as_str())
}

pub async fn preview_singbox_config(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
) -> Response {
    crate::blocking::response(move || match generated_node_config(&app, node_id) {
        Ok(config) => Json(json!({"node_id":node_id,"config":config})).into_response(),
        Err(response) => response,
    })
    .await
}

pub(super) fn enqueue_generated_config(
    app: &Shared,
    node_id: i64,
    method: &str,
    content: String,
) -> Response {
    match enqueue_generated_config_id(app, node_id, method, content) {
        Ok(command_id) => {
            (StatusCode::ACCEPTED, Json(json!({"command_id":command_id,"status":"pending"}))).into_response()
        }
        Err(EnqueueFailure::TooLarge) => api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "SINGBOX_CONFIG_TOO_LARGE",
            "generated config exceeds 1 MiB",
        ),
        Err(EnqueueFailure::Offline) => {
            api_error(StatusCode::CONFLICT, "AGENT_OFFLINE", "Agent is currently offline")
        }
        Err(EnqueueFailure::QueueFull) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "COMMAND_QUEUE_FULL", "command queue is full")
        }
        Err(EnqueueFailure::Unavailable) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "AGENT_UNAVAILABLE",
            "Agent queue is full or disconnected",
        ),
    }
}

impl EnqueueFailure {
    fn as_str(&self) -> &'static str {
        match self {
            Self::TooLarge => "config_too_large",
            Self::Offline => "offline",
            Self::QueueFull => "queue_full",
            Self::Unavailable => "agent_unavailable",
        }
    }
}

pub(super) fn enqueue_generated_config_id(
    app: &App,
    node_id: i64,
    method: &str,
    content: String,
) -> Result<String, EnqueueFailure> {
    if content.len() > proxy::MAX_GENERATED_CONFIG {
        return Err(EnqueueFailure::TooLarge);
    }
    let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
    let Some(agent) = agents.get(&node_id) else { return Err(EnqueueFailure::Offline) };
    let session = agent.session;
    let id =
        app.commands.start(node_id, session, method).map_err(|StartError::Full| EnqueueFailure::QueueFull)?;
    let message = json!({
        "type": "command",
        "id": id,
        "action": method,
        "params": {"content": content},
    })
    .to_string();
    match agent.tx.try_send(message) {
        Ok(()) => Ok(id),
        Err(_) => {
            app.commands.cancel(&id, node_id, session);
            Err(EnqueueFailure::Unavailable)
        }
    }
}

pub async fn check_singbox_config(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: axum::body::Bytes,
) -> Response {
    crate::blocking::response(move || {
        if !body.is_empty() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "config is generated from database state",
            );
        }
        match generated_node_config(&app, node_id) {
            Ok(config) => enqueue_generated_config(&app, node_id, "singbox.config.check", config.to_string()),
            Err(response) => response,
        }
    })
    .await
}

pub async fn apply_singbox_config(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: axum::body::Bytes,
) -> Response {
    crate::blocking::response(move || {
        if !body.is_empty() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "config is generated from database state",
            );
        }
        match generated_node_config(&app, node_id) {
            Ok(config) => enqueue_generated_config(&app, node_id, "singbox.config.apply", config.to_string()),
            Err(response) => response,
        }
    })
    .await
}
