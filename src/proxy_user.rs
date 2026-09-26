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
    is_system: bool,
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
            is_system: user.is_system,
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
    let current = match app.db.proxy_user(id) {
        Ok(Some(user)) => user,
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => return no_store(api::fail(error)),
    };
    if current.is_system && request.name.trim() != current.name {
        return no_store(api::answer(StatusCode::CONFLICT, "系统代理用户名称不能修改"));
    }
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
    if current.is_system {
        return no_store(api::answer(StatusCode::CONFLICT, "系统代理用户不能删除"));
    }
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
        assert_eq!(user["is_system"], false);
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

        let listed = list_proxy_users(Admin, State(app.clone())).await;
        let listed = response_json(listed).await;
        let listed_user = listed["users"].as_array().unwrap().iter().find(|user| user["id"] == id).unwrap();
        assert_eq!(listed_user["name"], "Alice 2");
        assert_eq!(listed_user["uuid"], regenerated["uuid"]);
        let listed_admin =
            listed["users"].as_array().unwrap().iter().find(|user| user["name"] == "admin").unwrap();
        assert_eq!(listed_admin["is_system"], true);

        let admin = app.db.proxy_users().unwrap().into_iter().find(|user| user.is_system).unwrap();
        assert_eq!(admin.name, "admin");
        assert!(admin.enabled);
        assert!(admin.proxy_node_ids.is_empty());

        let deleted = delete_proxy_user(Admin, State(app), Path(id)).await;
        assert_eq!(deleted.status(), StatusCode::OK);
        assert_eq!(response_json(deleted).await["deleted"], true);
    }

    #[tokio::test]
    async fn system_proxy_user_api_blocks_delete_and_rename_but_allows_edit_and_regenerate() {
        let app = app();
        let admin = app.db.proxy_users().unwrap().into_iter().find(|user| user.is_system).unwrap();
        let id = admin.id;
        let old_uuid = admin.uuid;

        let deleted = delete_proxy_user(Admin, State(app.clone()), Path(id)).await;
        assert_eq!(deleted.status(), StatusCode::CONFLICT);
        assert_eq!(app.db.proxy_user(id).unwrap().unwrap().name, "admin");

        let renamed = update_proxy_user(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(UpdateProxyUserRequest {
                name: "renamed".into(),
                enabled: true,
                note: String::new(),
                proxy_node_ids: Vec::new(),
            })),
        )
        .await;
        assert_eq!(renamed.status(), StatusCode::CONFLICT);
        assert_eq!(app.db.proxy_user(id).unwrap().unwrap().name, "admin");

        let edited = update_proxy_user(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(UpdateProxyUserRequest {
                name: "admin".into(),
                enabled: false,
                note: "paused by operator".into(),
                proxy_node_ids: Vec::new(),
            })),
        )
        .await;
        assert_eq!(edited.status(), StatusCode::OK);
        let edited = response_json(edited).await["user"].clone();
        assert_eq!(edited["name"], "admin");
        assert_eq!(edited["enabled"], false);
        assert_eq!(edited["note"], "paused by operator");
        assert_eq!(edited["is_system"], true);

        let regenerated = regenerate_proxy_user(Admin, State(app.clone()), Path(id)).await;
        assert_eq!(regenerated.status(), StatusCode::OK);
        let regenerated = response_json(regenerated).await["user"].clone();
        assert_ne!(regenerated["uuid"], old_uuid);
        assert_eq!(regenerated["is_system"], true);
        assert_eq!(app.db.proxy_user(id).unwrap().unwrap().uuid, regenerated["uuid"]);
    }

    #[test]
    fn create_proxy_user_request_does_not_accept_system_identity() {
        let parsed = serde_json::from_value::<CreateProxyUserRequest>(json!({
            "name": "forged-system",
            "enabled": true,
            "is_system": true
        }));
        assert!(parsed.is_err());

        let parsed = serde_json::from_value::<UpdateProxyUserRequest>(json!({
            "name": "admin",
            "enabled": true,
            "is_system": false
        }));
        assert!(parsed.is_err());
    }
}
