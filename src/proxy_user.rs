//! 管理代理用户与 ProxyNode 授权关系；管理员账号仍由现有认证模块处理。

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use crate::api::{self, Admin};
use crate::db::{ProxyUser, ProxyUserCreateSettings, ProxyUserLimits};
use crate::proxy_deploy;
use crate::proxy_provision;
use crate::Shared;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyUserRequest {
    pub name: String,
    pub password: String,
    pub enabled: bool,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub proxy_node_ids: Vec<i64>,
    #[serde(default)]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(default)]
    pub traffic_reset_day: Option<u8>,
    #[serde(default, deserialize_with = "deserialize_expire_date")]
    pub expire_date: Option<Option<String>>,
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
    #[serde(default)]
    pub traffic_limit_bytes: Option<i64>,
    #[serde(default)]
    pub traffic_reset_day: Option<u8>,
    #[serde(default, deserialize_with = "deserialize_expire_date")]
    pub expire_date: Option<Option<String>>,
}

fn deserialize_expire_date<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
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
    traffic_limit_bytes: i64,
    traffic_reset_day: u8,
    expire_at: Option<i64>,
    expire_date: Option<String>,
    traffic: ProxyUserTrafficResponse,
    access_state: crate::proxy_access::ProxyUserAccessState,
}

#[derive(Serialize)]
struct ProxyUserTrafficResponse {
    uplink_bytes: i64,
    downlink_bytes: i64,
    used_bytes: i64,
}

impl ProxyUserResponse {
    fn from_user(user: ProxyUser, now: i64) -> Self {
        let used_bytes = user.used_bytes();
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
            traffic_limit_bytes: user.traffic_limit_bytes,
            traffic_reset_day: user.traffic_reset_day,
            expire_at: user.expire_at,
            expire_date: user.expire_at.and_then(crate::hub_time::expiry_timestamp_date),
            traffic: ProxyUserTrafficResponse {
                uplink_bytes: user.uplink_bytes,
                downlink_bytes: user.downlink_bytes,
                used_bytes,
            },
            access_state: crate::proxy_access::effective_proxy_access(
                user.enabled,
                user.expire_at,
                user.traffic_limit_bytes,
                user.uplink_bytes,
                user.downlink_bytes,
                now,
            ),
        }
    }
}

fn parse_expire_date(value: Option<&str>) -> Result<Option<i64>, &'static str> {
    value
        .map(|value| {
            let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| "到期日期格式无效")?;
            crate::hub_time::expiry_date_timestamp(date).map_err(|_| "到期日期超出可用范围")
        })
        .transpose()
}

pub async fn list_proxy_users(_: Admin, State(app): State<Shared>) -> Response {
    match app.db.proxy_users() {
        Ok(users) => {
            let now = crate::hub_time::now_timestamp();
            no_store(Json(json!({ "users": users.into_iter().map(|user| ProxyUserResponse::from_user(user, now)).collect::<Vec<_>>() })).into_response())
        }
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
    if !(12..=1024).contains(&request.password.len()) {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "登录密码长度须为 12 到 1024 字节"));
    }
    let traffic_limit_bytes = request.traffic_limit_bytes.unwrap_or(0);
    let traffic_reset_day = request.traffic_reset_day.unwrap_or(1);
    let expire_date = request.expire_date.unwrap_or(None);
    let expire_at = match parse_expire_date(expire_date.as_deref()) {
        Ok(expire_at) => expire_at,
        Err(message) => return no_store(api::answer(StatusCode::BAD_REQUEST, message)),
    };
    if traffic_limit_bytes < 0 || !(1..=28).contains(&traffic_reset_day) {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "流量限额或每月重置日无效"));
    }
    let password_hash = match crate::auth::hash_password(&request.password) {
        Ok(hash) => hash,
        Err(error) => return no_store(api::fail(error)),
    };
    let uuid = proxy_provision::uuid_v4();
    match app.db.create_proxy_user_with_password(
        &request.name,
        &uuid,
        request.enabled,
        &request.note,
        &request.proxy_node_ids,
        ProxyUserCreateSettings {
            limits: ProxyUserLimits { traffic_limit_bytes, traffic_reset_day, expire_at },
            password_hash: Some(&password_hash),
        },
    ) {
        Ok((user, server_ids)) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            let response_user = ProxyUserResponse::from_user(user, crate::hub_time::now_timestamp());
            no_store(
                (
                    StatusCode::CREATED,
                    Json(json!({
                        "user": response_user,
                        "failed_servers": failures
                    })),
                )
                    .into_response(),
            )
        }
        Err(error) => no_store(api::fail(error)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetProxyUserPasswordRequest {
    password: String,
}

/// 管理员单独重设普通代理用户登录密码，不把密码混入通用用户更新请求。
pub async fn set_proxy_user_password(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<SetProxyUserPasswordRequest>, JsonRejection>,
) -> Response {
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let Ok(Json(request)) = body else {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "密码请求格式不正确"));
    };
    if !(12..=1024).contains(&request.password.len()) {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "登录密码长度须为 12 到 1024 字节"));
    }
    match app.db.proxy_user(id) {
        Ok(Some(user)) if user.is_system => {
            return no_store(api::answer(StatusCode::CONFLICT, "系统用户使用 Monitor 管理员密码登录"));
        }
        Ok(Some(_)) => {}
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => return no_store(api::fail(error)),
    }
    let password_hash = match crate::auth::hash_password(&request.password) {
        Ok(hash) => hash,
        Err(error) => return no_store(api::fail(error)),
    };
    match app.db.set_proxy_user_password_hash(id, &password_hash) {
        Ok(true) => no_store(Json(json!({ "updated": true })).into_response()),
        Ok(false) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
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
    let traffic_limit_bytes = request.traffic_limit_bytes.unwrap_or(current.traffic_limit_bytes);
    let traffic_reset_day = request.traffic_reset_day.unwrap_or(current.traffic_reset_day);
    let expire_at = match request.expire_date {
        Some(expire_date) => match parse_expire_date(expire_date.as_deref()) {
            Ok(expire_at) => expire_at,
            Err(message) => return no_store(api::answer(StatusCode::BAD_REQUEST, message)),
        },
        None => current.expire_at,
    };
    if traffic_limit_bytes < 0 || !(1..=28).contains(&traffic_reset_day) {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "流量限额或每月重置日无效"));
    }
    match app.db.update_proxy_user_settings(
        id,
        &request.name,
        request.enabled,
        &request.note,
        &request.proxy_node_ids,
        ProxyUserLimits { traffic_limit_bytes, traffic_reset_day, expire_at },
    ) {
        Ok(Some((user, server_ids))) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            let response_user = ProxyUserResponse::from_user(user, crate::hub_time::now_timestamp());
            no_store(Json(json!({ "user": response_user, "failed_servers": failures })).into_response())
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
            let response_user = ProxyUserResponse::from_user(user, crate::hub_time::now_timestamp());
            no_store(Json(json!({ "user": response_user, "failed_servers": failures })).into_response())
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
    let response_user = ProxyUserResponse::from_user(user, crate::hub_time::now_timestamp());
    no_store(Json(json!({ "user": response_user, "failed_servers": failures })).into_response())
}

pub async fn reset_proxy_user_traffic(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    if id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    let lock = app.proxy_user_lock(id);
    let _operation = lock.lock().await;
    match app.db.reset_proxy_user_traffic(id, crate::hub_time::now_timestamp()) {
        Ok(Some((user, server_ids))) => {
            let failures = proxy_deploy::deploy_proxy_user_servers(&app, &server_ids).await;
            let response_user = ProxyUserResponse::from_user(user, crate::hub_time::now_timestamp());
            no_store(Json(json!({ "user": response_user, "failed_servers": failures })).into_response())
        }
        Ok(None) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => no_store(api::fail(error)),
    }
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
                "user": ProxyUserResponse::from_user(disabled, crate::hub_time::now_timestamp()),
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
                password: "a-secure-user-password".into(),
                enabled: true,
                note: "first note".into(),
                proxy_node_ids: Vec::new(),
                traffic_limit_bytes: Some(0),
                traffic_reset_day: Some(1),
                expire_date: Some(None),
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
        assert_eq!(user["traffic_limit_bytes"], 0);
        assert_eq!(user["traffic_reset_day"], 1);
        assert_eq!(user["expire_at"], serde_json::Value::Null);
        assert_eq!(user["expire_date"], serde_json::Value::Null);
        assert_eq!(user["traffic"]["used_bytes"], 0);
        assert!(user.get("password_hash").is_none());
        assert!(user.get("password").is_none());

        let updated = update_proxy_user(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(UpdateProxyUserRequest {
                name: "Alice 2".into(),
                enabled: false,
                note: "paused".into(),
                proxy_node_ids: Vec::new(),
                traffic_limit_bytes: Some(0),
                traffic_reset_day: Some(1),
                expire_date: Some(None),
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
    async fn proxy_user_password_is_hash_only_and_reset_revokes_existing_sessions() {
        let app = app();
        let created = create_proxy_user(
            Admin,
            State(app.clone()),
            Ok(Json(CreateProxyUserRequest {
                name: "password-user".into(),
                password: "initial-user-password".into(),
                enabled: true,
                note: String::new(),
                proxy_node_ids: Vec::new(),
                traffic_limit_bytes: None,
                traffic_reset_day: None,
                expire_date: None,
            })),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        let payload = response_json(created).await;
        let id = payload["user"]["id"].as_i64().unwrap();
        assert!(payload["user"].get("password_hash").is_none());
        let before = app.db.proxy_user_login_credential("password-user").unwrap().unwrap().2.unwrap();
        assert!(before.starts_with("$argon2"));
        assert!(crate::auth::verify_password("initial-user-password", &before));

        let now = crate::hub_time::now_timestamp();
        app.db.create_proxy_user_login_session("before-reset", id, false, &before, now, now + 60).unwrap();
        let reset = set_proxy_user_password(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(SetProxyUserPasswordRequest { password: "replacement-password".into() })),
        )
        .await;
        assert_eq!(reset.status(), StatusCode::OK);
        assert_eq!(app.db.proxy_user_session("before-reset", now).unwrap(), None);
        let after = app.db.proxy_user_login_credential("password-user").unwrap().unwrap().2.unwrap();
        assert_ne!(before, after);
        assert!(crate::auth::verify_password("replacement-password", &after));

        let system_id = app.db.proxy_users().unwrap().into_iter().find(|user| user.is_system).unwrap().id;
        let refused = set_proxy_user_password(
            Admin,
            State(app),
            Path(system_id),
            Ok(Json(SetProxyUserPasswordRequest { password: "replacement-password".into() })),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
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
                traffic_limit_bytes: None,
                traffic_reset_day: None,
                expire_date: None,
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
                traffic_limit_bytes: None,
                traffic_reset_day: None,
                expire_date: None,
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

    #[tokio::test]
    async fn quota_and_expiry_inputs_round_trip_as_bytes_and_hub_date() {
        let app = app();
        let request = CreateProxyUserRequest {
            name: "metered".into(),
            password: "a-secure-user-password".into(),
            enabled: true,
            note: String::new(),
            proxy_node_ids: Vec::new(),
            traffic_limit_bytes: Some(200 * 1024 * 1024 * 1024),
            traffic_reset_day: Some(28),
            expire_date: Some(Some("2099-10-25".into())),
        };
        let response = create_proxy_user(Admin, State(app.clone()), Ok(Json(request))).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let user_json = response_json(response).await["user"].clone();
        assert_eq!(user_json["traffic_limit_bytes"], 200_i64 * 1024 * 1024 * 1024);
        assert_eq!(user_json["traffic_reset_day"], 28);
        assert_eq!(user_json["expire_date"], "2099-10-25");
        assert_eq!(user_json["access_state"], "enabled");
        let id = user_json["id"].as_i64().unwrap();
        assert_eq!(
            app.db.proxy_user(id).unwrap().unwrap().expire_at,
            crate::hub_time::expiry_date_timestamp(NaiveDate::from_ymd_opt(2099, 10, 25).unwrap()).ok()
        );

        let invalid = update_proxy_user(
            Admin,
            State(app.clone()),
            Path(id),
            Ok(Json(UpdateProxyUserRequest {
                name: "metered".into(),
                enabled: true,
                note: String::new(),
                proxy_node_ids: Vec::new(),
                traffic_limit_bytes: Some(0),
                traffic_reset_day: Some(29),
                expire_date: None,
            })),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let still_set = app.db.proxy_user(id).unwrap().unwrap();
        assert_eq!(still_set.traffic_limit_bytes, 200 * 1024 * 1024 * 1024);
        assert_eq!(still_set.traffic_reset_day, 28);
        assert!(still_set.expire_at.is_some());
    }

    #[tokio::test]
    async fn traffic_reset_endpoint_clears_system_users_and_returns_not_found_for_missing_users() {
        let app = app();
        let admin = app.db.proxy_users().unwrap().into_iter().find(|user| user.is_system).unwrap();
        let before = app.db.proxy_user(admin.id).unwrap().unwrap().reset_generation;
        let response = reset_proxy_user_traffic(Admin, State(app.clone()), Path(admin.id)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["user"]["traffic"]["used_bytes"], 0);
        assert_eq!(app.db.proxy_user(admin.id).unwrap().unwrap().reset_generation, before + 1);

        let missing = reset_proxy_user_traffic(Admin, State(app), Path(i64::MAX)).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
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

        let legacy = serde_json::from_value::<UpdateProxyUserRequest>(json!({
            "name": "admin",
            "enabled": true
        }))
        .unwrap();
        assert_eq!(legacy.traffic_limit_bytes, None, "旧客户端缺省字段不会意外重置现有配置");
        assert_eq!(legacy.expire_date, None);
        let clear_expiry = serde_json::from_value::<UpdateProxyUserRequest>(json!({
            "name": "admin",
            "enabled": true,
            "expire_date": null
        }))
        .unwrap();
        assert_eq!(clear_expiry.expire_date, Some(None), "显式 null 表示清除到期日");
    }
}
