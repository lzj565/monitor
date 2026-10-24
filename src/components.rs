use crate::{
    agent_ws,
    api::{self, Admin},
    db::InstallChoices,
    Shared,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
fn with_online(app: &Shared, id: i64, mut profile: serde_json::Value) -> serde_json::Value {
    profile["online"] = json!(app.agents.read().unwrap_or_else(|e| e.into_inner()).contains_key(&id));
    profile
}
pub async fn get(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.component_profile(id) {
        Ok(Some(p)) => {
            ([(header::CACHE_CONTROL, "no-store")], Json(with_online(&app, id, p))).into_response()
        }
        Ok(None) => api::answer(StatusCode::NOT_FOUND, "节点不存在"),
        Err(e) => api::fail(e),
    })
    .await
}
pub async fn save(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<InstallChoices>, axum::extract::rejection::JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let Ok(Json(choices)) = body else {
            return api::answer(StatusCode::BAD_REQUEST, "安装选项格式不正确");
        };
        match app.db.save_component_choices(id, &choices) {
            Ok(true) => match app.db.component_profile(id) {
                Ok(Some(p)) => {
                    ([(header::CACHE_CONTROL, "no-store")], Json(with_online(&app, id, p))).into_response()
                }
                Ok(None) => api::answer(StatusCode::NOT_FOUND, "节点不存在"),
                Err(e) => api::fail(e),
            },
            Ok(false) => api::answer(StatusCode::NOT_FOUND, "节点不存在"),
            Err(e) => api::fail(e),
        }
    })
    .await
}
/// A strict, parser-free shell format; authenticates only the requesting node.
pub async fn installer(State(app): State<Shared>, headers: HeaderMap) -> Response {
    crate::blocking::response(move || {
        let id = match agent_ws::authenticate(&app, &headers) {
            Ok(id) => id,
            Err(e) => return api::answer(StatusCode::UNAUTHORIZED, e),
        };
        match app.db.component_choices(id) {
            Ok(Some(c)) => (
                [(header::CACHE_CONTROL, "no-store")],
                format!("singbox=1\nrealm={}\nmieru={}\n", u8::from(c.realm), u8::from(c.mieru)),
            )
                .into_response(),
            Ok(None) => api::answer(StatusCode::NOT_FOUND, "component profile not found"),
            Err(e) => api::fail(e),
        }
    })
    .await
}
