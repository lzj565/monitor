//! 管理代理用户与 ProxyNode 授权关系；管理员账号仍由现有认证模块处理。

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::{self, Admin};
use crate::db::ProxyUser;
use crate::proxy_deploy;
use crate::proxy_provision;
use crate::Shared;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyUserRequest {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub proxy_node_ids: Vec<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProxyUserRequest {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub proxy_node_ids: Vec<i64>,
}

#[derive(Serialize)]
struct ProxyUserResponse {
    id: i64,
    name: String,
    uuid: String,
    enabled: bool,
    note: String,
    proxy_node_ids: Vec<i64>,
    created_at: i64,
    updated_at: i64,
}

impl From<ProxyUser> for ProxyUserResponse {
    fn from(user: ProxyUser) -> Self {
        Self {
            id: user.id,
            name: user.name,
            uuid: user.uuid,
            enabled: user.enabled,
            note: user.note,
            proxy_node_ids: user.proxy_node_ids,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}

pub async fn list_proxy_users(_: Admin, State(app): State<Shared>) -> Response {
    match app.db.proxy_users() {
        Ok(users) => no_store(
            Json(json!({ "users": users.into_iter().map(ProxyUserResponse::from).collect::<Vec<_>>() }))
                .into_response(),
        ),
        Err(error) => no_store(api::fail(error)),
    }
}

pub async fn create_proxy_user(
    _: Admin,
    State(app): State<Shared>,
    body: Result<Json<CreateProxyUserRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "用户请求格式不正确"));
    };
    let uuid = proxy_provision::uuid_v4();
    match app.db.create_proxy_user_with_nodes(
        &request.name,
        &uuid,
        request.enabled,
        &request.note,
        &request.proxy_node_ids,
    ) {
        Ok((user, server_ids)) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            no_store(
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "user": ProxyUserResponse::from(user),
                        "failed_servers": failures
                    })),
                )
                    .into_response(),
            )
        }
        Err(error) => no_store(api::fail(error)),
    }
}

pub async fn update_proxy_user(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<UpdateProxyUserRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "用户请求格式不正确"));
    };
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let lock = app.proxy_user_lock(id);
    let _operation = lock.lock().await;
    match app.db.update_proxy_user_profile(
        id,
        &request.name,
        request.enabled,
        &request.note,
        &request.proxy_node_ids,
    ) {
        Ok(Some((user, server_ids))) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            no_store(
                Json(json!({ "user": ProxyUserResponse::from(user), "failed_servers": failures }))
                    .into_response(),
            )
        }
        Ok(None) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => no_store(api::fail(error)),
    }
}

pub async fn regenerate_proxy_user(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let lock = app.proxy_user_lock(id);
    let _operation = lock.lock().await;
    match app.db.regenerate_proxy_user_uuid(id, &proxy_provision::uuid_v4()) {
        Ok(Some((user, server_ids))) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            no_store(
                Json(json!({ "user": ProxyUserResponse::from(user), "failed_servers": failures }))
                    .into_response(),
            )
        }
        Ok(None) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => no_store(api::fail(error)),
    }
}

pub async fn sync_proxy_user(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let lock = app.proxy_user_lock(id);
    let _operation = lock.lock().await;
    let user = match app.db.proxy_user(id) {
        Ok(Some(user)) => user,
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => return no_store(api::fail(error)),
    };
    let server_ids = match app.db.proxy_user_server_ids(id) {
        Ok(ids) => ids,
        Err(error) => return no_store(api::fail(error)),
    };
    let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
    no_store(
        Json(json!({ "user": ProxyUserResponse::from(user), "failed_servers": failures })).into_response(),
    )
}

pub async fn delete_proxy_user(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let lock = app.proxy_user_lock(id);
    let _operation = lock.lock().await;
    let current = match app.db.proxy_user(id) {
        Ok(Some(user)) => user,
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => return no_store(api::fail(error)),
    };
    let Some((disabled, server_ids)) = (match app.db.update_proxy_user_profile(
        id,
        &current.name,
        false,
        &current.note,
        &current.proxy_node_ids,
    ) {
        Ok(updated) => updated,
        Err(error) => return no_store(api::fail(error)),
    }) else {
        return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在"));
    };
    let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
    if !failures.is_empty() {
        return no_store(
            Json(json!({
                "deleted": false,
                "user": ProxyUserResponse::from(disabled),
                "failed_servers": failures
            }))
            .into_response(),
        );
    }
    match app.db.delete_proxy_user(id) {
        Ok(true) => {
            no_store(Json(json!({ "deleted": true, "user": null, "failed_servers": [] })).into_response())
        }
        Ok(false) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => no_store(api::fail(error)),
    }
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::to_bytes;

    use crate::db::Db;
    use crate::App;
    use std::sync::Arc;

    fn app() -> crate::Shared {
        Arc::new(App::for_test(Db::open(":memory:").unwrap()))
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn admin_user_crud_generates_and_regenerates_uuid_without_exposing_legacy_billing_fields() {
        let app = app();
        let created = create_proxy_user(
            Admin,
            State(app.clone()),
            Ok(Json(CreateProxyUserRequest {
                name: " Alice ".into(),
                enabled: true,
                note: "first note".into(),
                proxy_node_ids: Vec::new(),
            })),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        assert_eq!(created.headers()[header::CACHE_CONTROL], "no-store");
        let created = response_json(created).await;
        let user = created["user"].clone();
        let id = user["id"].as_i64().unwrap();
        let first_uuid = user["uuid"].as_str().unwrap();
        assert!(crate::proxy_config::is_uuid(first_uuid));
        assert_eq!(user["name"], "Alice");
        assert_eq!(user["note"], "first note");
        assert_eq!(user["proxy_node_ids"], json!([]));
        assert!(user.get("quota").is_none());
        assert!(user.get("expire_at").is_none());

        let updated = update_proxy_user(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(UpdateProxyUserRequest {
                name: "Alice 2".into(),
                enabled: false,
                note: "paused".into(),
                proxy_node_ids: Vec::new(),
            })),
        )
        .await;
        let updated = response_json(updated).await["user"].clone();
        assert_eq!(updated["name"], "Alice 2");
        assert_eq!(updated["enabled"], false);
        assert_eq!(updated["uuid"], first_uuid);

        let regenerated = regenerate_proxy_user(Admin, State(app.clone()), Path(id)).await;
        let regenerated = response_json(regenerated).await["user"].clone();
        assert!(crate::proxy_config::is_uuid(regenerated["uuid"].as_str().unwrap()));
        assert_ne!(regenerated["uuid"], first_uuid);
        assert_eq!(regenerated["enabled"], false, "regenerate 只换 UUID，不改其他 desired state");

        let listed = list_proxy_users(Admin, State(app)).await;
        let listed = response_json(listed).await;
        assert_eq!(listed["users"][0]["name"], "Alice 2");
        assert_eq!(listed["users"][0]["uuid"], regenerated["uuid"]);
    }
}
