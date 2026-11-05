use super::*;

fn rejected() -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "INVALID_EXTERNAL_PROXY",
        "外部代理配置不合法、名称重复或列表已变更，请检查后重试",
    )
}
pub async fn list_external(_: ApiAdmin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.external_proxies() {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(e) => api_internal(e),
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalPreview {
    yaml: String,
    exclude_id: Option<i64>,
}
pub async fn preview_external(
    _: ApiAdmin,
    State(app): State<Shared>,
    Json(body): Json<ExternalPreview>,
) -> Response {
    crate::blocking::response(move || {
        let (items, ignored) = match crate::external_proxy::parse_yaml(&body.yaml) {
            Ok(v) => v,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "INVALID_YAML", e.to_string()),
        };
        let existing = match app.db.external_proxies() {
            Ok(p) => p,
            Err(e) => return api_internal(e),
        };
        let mut names = std::collections::HashSet::new();
        let rows: Vec<_> = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let checked = crate::external_proxy::validate(&item);
                let error = match &checked {
                    Err(e) => Some(e.to_string()),
                    Ok(config) => {
                        let name = config["name"].as_str().unwrap_or_default();
                        if !names.insert(name.to_owned())
                            || existing.iter().any(|p| p.name == name && Some(p.id) != body.exclude_id)
                        {
                            Some("名称重复，请修改后重新校验".into())
                        } else {
                            None
                        }
                    }
                };
                json!({"index":index,"config":checked.unwrap_or(item),"error":error})
            })
            .collect();
        Json(json!({"items":rows,"ignored_sections":ignored})).into_response()
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalImport {
    items: Vec<Value>,
}
pub async fn import_external(
    _: ApiAdmin,
    State(app): State<Shared>,
    Json(body): Json<ExternalImport>,
) -> Response {
    crate::blocking::response(move || {
        if serde_json::to_vec(&body.items).map_or(true, |v| v.len() > crate::external_proxy::MAX_YAML_BYTES) {
            return rejected();
        }
        match app.db.import_external_proxies(&body.items) {
            Ok(ids) => (StatusCode::CREATED, Json(json!({"ids":ids}))).into_response(),
            Err(_) => rejected(),
        }
    })
    .await
}
pub async fn get_external(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.external_proxy_config(id) {
        Ok(Some(config)) => Json(json!({"config":config})).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "EXTERNAL_PROXY_NOT_FOUND", "外部代理不存在"),
        Err(e) => api_internal(e),
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalUpdate {
    config: Option<Value>,
    enabled: Option<bool>,
}
pub async fn update_external(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<ExternalUpdate>,
) -> Response {
    crate::blocking::response(move || {
        match app.db.update_external_proxy(id, body.config.as_ref(), body.enabled) {
            Ok(true) => Json(json!({"ok":true})).into_response(),
            Ok(false) => api_error(StatusCode::NOT_FOUND, "EXTERNAL_PROXY_NOT_FOUND", "外部代理不存在"),
            Err(_) => rejected(),
        }
    })
    .await
}
pub async fn delete_external(_: ApiAdmin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.delete_external_proxy(id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "EXTERNAL_PROXY_NOT_FOUND", "外部代理不存在"),
        Err(e) => api_internal(e),
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAccess {
    ids: Vec<i64>,
}
pub async fn get_external_access(_: ApiAdmin, State(app): State<Shared>, Path(user): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.external_authorizations(user) {
        Ok(ids) => Json(json!({"ids":ids})).into_response(),
        Err(_) => api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "用户不存在"),
    })
    .await
}
pub async fn save_external_access(
    _: ApiAdmin,
    State(app): State<Shared>,
    Path(user): Path<i64>,
    Json(body): Json<ExternalAccess>,
) -> Response {
    crate::blocking::response(move || match app.db.replace_external_authorizations(user, &body.ids) {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(_) => rejected(),
    })
    .await
}

#[derive(Deserialize)]
pub struct EntryOrder {
    ids: Vec<Value>,
}
pub async fn reorder_user_entries(
    auth: UserAuth,
    state: State<Shared>,
    Json(body): Json<EntryOrder>,
) -> Response {
    if body.ids.iter().all(Value::is_i64) {
        return reorder_user_proxies(
            auth,
            state,
            Json(Order { ids: body.ids.iter().filter_map(Value::as_i64).collect() }),
        )
        .await;
    }
    let keys = body.ids.into_iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>();
    let Some(keys) = keys else { return rejected() };
    crate::blocking::response(move || {
        match state.0.db.reorder_user_entries(auth.0, &keys, Utc::now().timestamp()) {
            Ok(()) => Json(json!({"ok":true})).into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}
pub async fn save_user_entry_preference(
    UserAuth(user): UserAuth,
    State(app): State<Shared>,
    Path(key): Path<String>,
    Json(pref): Json<EntryPreference>,
) -> Response {
    // Numeric paths from older clients still address internal entries only.
    if let Ok(id) = key.parse::<i64>() {
        return save_user_entry_preferences(UserAuth(user), State(app), Path(id), Json(pref)).await;
    }
    crate::blocking::response(move || {
        match app.db.save_user_entry_preference(user, &key, pref.hidden, pref.custom_name) {
            Ok(()) => Json(json!({"ok":true})).into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}

pub async fn external_no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    response
}
