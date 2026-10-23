//! The panel and public-status HTTP surface.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::agent_ws::Agent;
use crate::auth::{
    authed, client_ip, current_session, hash_password, issue_session, issued_at, random_token, with_cookies,
};
use crate::command::{self, StartError};
use crate::db::{
    self, AuthorizationDraft, AuthorizationWriteIssue, Db, Node, NodePatch, PingTask, Proxy, ProxyWriteIssue,
    Traffic, TrafficPatch, User, UserDeleteResult, UserWriteIssue,
};
use crate::proxy::{self, AuthorizationRequest, ProxyRequest, UserRequest};
use crate::{agent_ws, App, Shared};

/// Present only on requests carrying a valid session. Handlers taking it cannot
/// be reached unauthenticated, so the check cannot be omitted.
pub struct Admin;

impl FromRequestParts<Shared> for Admin {
    type Rejection = Response;

    #[allow(clippy::result_large_err)]
    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let app = app.clone();
        let headers = parts.headers.clone();
        crate::blocking::run(move || {
            if authed(&app, &headers) {
                Ok(Admin)
            } else {
                Err(answer(StatusCode::UNAUTHORIZED, "登录已失效，请重新登录"))
            }
        })
        .await
        .map_err(fail)?
    }
}

/// Admin-only authentication for the V1 resource API, whose errors use the
/// structured JSON envelope instead of the legacy text response.
pub struct ApiAdmin;

impl FromRequestParts<Shared> for ApiAdmin {
    type Rejection = Response;

    #[allow(clippy::result_large_err)]
    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let app = app.clone();
        let headers = parts.headers.clone();
        crate::blocking::run(move || {
            if authed(&app, &headers) {
                Ok(ApiAdmin)
            } else {
                Err(api_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "登录已失效，请重新登录"))
            }
        })
        .await
        .map_err(fail)?
    }
}

/// The current user's id, resolved from a live session. Administrators access
/// only the built-in `admin` user; endpoints never accept a user id from the caller.
pub struct UserAuth(pub i64);

impl FromRequestParts<Shared> for UserAuth {
    type Rejection = Response;

    #[allow(clippy::result_large_err)]
    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let app = app.clone();
        let headers = parts.headers.clone();
        crate::blocking::run(move || {
            if let Some(hash) = current_session(&headers) {
                if let Some(id) = app.db.user_session_user_id(&hash) {
                    return Ok(UserAuth(id));
                }
                if app.db.admin_session_valid(&hash) {
                    return match app.db.user_login_record("admin") {
                        Ok(Some(user)) => Ok(UserAuth(user.id)),
                        Ok(None) => {
                            Err(api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "内置 admin 用户不存在"))
                        }
                        Err(error) => Err(api_internal(error)),
                    };
                }
            }
            Err(api_error(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "登录已失效，请重新登录"))
        })
        .await
        .map_err(fail)?
    }
}

/// Marks a response whose text was written for the reader; see [`plain_errors`].
#[derive(Clone)]
struct Written;

/// An error response carrying `text` as written. Every error the hub composes
/// is built here.
pub(crate) fn answer(status: StatusCode, text: impl Into<String>) -> Response {
    let mut response = (status, text.into()).into_response();
    response.extensions_mut().insert(Written);
    response
}

/// What a failure this hub cannot explain to the reader says instead.
pub(crate) const INTERNAL: &str = "hub 内部出错，详细原因见 hub 日志";

/// Answers an error: 400 with the outermost [`crate::Shown`] message in its
/// chain, or, without one, 500 with [`INTERNAL`]. The chain is logged in full
/// either way, the underlying cause of a shown message included.
pub(crate) fn fail(e: impl Into<anyhow::Error>) -> Response {
    let e = e.into();
    match e.downcast_ref::<crate::Shown>() {
        Some(shown) => {
            info!("request refused: {e:#}");
            answer(StatusCode::BAD_REQUEST, shown.0.clone())
        }
        None => {
            warn!("request failed: {e:#}");
            answer(StatusCode::INTERNAL_SERVER_ERROR, INTERNAL)
        }
    }
}

fn bad(message: &str) -> Response {
    answer(StatusCode::BAD_REQUEST, message)
}

fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let mut response =
        (status, Json(json!({"error":{"code":code,"message":message.into()}}))).into_response();
    response.extensions_mut().insert(Written);
    response
}

fn api_internal(error: impl Into<anyhow::Error>) -> Response {
    let error = error.into();
    warn!("resource API request failed: {error:#}");
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", INTERNAL)
}

#[allow(clippy::result_large_err)]
fn api_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, Response> {
    body.map(|Json(value)| value)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "INVALID_REQUEST", "request body must be valid JSON"))
}

fn api_input_error(error: proxy::InputError) -> Response {
    api_error(StatusCode::BAD_REQUEST, error.code, error.message)
}

#[allow(clippy::result_large_err)]
fn api_node_exists(app: &Shared, node_id: i64) -> Result<(), Response> {
    match app.db.node(node_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(api_error(StatusCode::NOT_FOUND, "NODE_NOT_FOUND", "node does not exist")),
        Err(error) => Err(api_internal(error)),
    }
}

#[allow(clippy::result_large_err)]
fn api_proxy(app: &Shared, id: i64) -> Result<Proxy, Response> {
    match app.db.proxy(id) {
        Ok(Some(proxy)) => Ok(proxy),
        Ok(None) => Err(api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist")),
        Err(error) => Err(api_internal(error)),
    }
}

#[allow(clippy::result_large_err)]
fn api_user(app: &Shared, id: i64) -> Result<User, Response> {
    match app.db.user(id) {
        Ok(Some(user)) => Ok(user),
        Ok(None) => Err(api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist")),
        Err(error) => Err(api_internal(error)),
    }
}

fn map_proxy_issue(issue: ProxyWriteIssue) -> Response {
    match issue {
        ProxyWriteIssue::NodeNotFound => {
            api_error(StatusCode::NOT_FOUND, "NODE_NOT_FOUND", "node does not exist")
        }
        ProxyWriteIssue::ProxyNotFound => {
            api_error(StatusCode::NOT_FOUND, "PROXY_NOT_FOUND", "proxy does not exist")
        }
        ProxyWriteIssue::PortConflict => api_error(
            StatusCode::CONFLICT,
            "PROXY_PORT_CONFLICT",
            "an enabled proxy on this node already uses the port",
        ),
        ProxyWriteIssue::NameConflict => {
            api_error(StatusCode::CONFLICT, "PROXY_NAME_CONFLICT", "生成后的代理名称已被使用")
        }
    }
}

fn map_user_issue(issue: UserWriteIssue) -> Response {
    match issue {
        UserWriteIssue::UserNotFound => {
            api_error(StatusCode::NOT_FOUND, "USER_NOT_FOUND", "user does not exist")
        }
        UserWriteIssue::UsernameConflict => {
            api_error(StatusCode::CONFLICT, "USERNAME_CONFLICT", "username is already in use")
        }
        UserWriteIssue::AdminUsernameLocked => api_error(
            StatusCode::CONFLICT,
            "SYSTEM_USER_NAME_LOCKED",
            "the default admin username cannot be changed",
        ),
        UserWriteIssue::PasswordRequired => {
            api_error(StatusCode::BAD_REQUEST, "PASSWORD_REQUIRED", "请设置用户登录密码")
        }
    }
}

/// Converts framework errors inside the resource router to the same envelope
/// as handler-created failures. The outer legacy mapper sees [`Written`] and
/// leaves this JSON intact.
pub async fn api_json_errors(response: Response) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || response.extensions().get::<Written>().is_some()
    {
        return response;
    }
    let (code, message) = match status {
        StatusCode::UNAUTHORIZED => ("UNAUTHORIZED", "登录已失效，请重新登录"),
        StatusCode::NOT_FOUND => ("NOT_FOUND", "请求的内容不存在"),
        StatusCode::CONFLICT => ("CONFLICT", "请求与当前资源状态冲突"),
        StatusCode::PAYLOAD_TOO_LARGE => ("PAYLOAD_TOO_LARGE", "提交的内容过大"),
        StatusCode::METHOD_NOT_ALLOWED => ("METHOD_NOT_ALLOWED", "不支持该请求方法"),
        s if s.is_server_error() => ("INTERNAL_ERROR", INTERNAL),
        _ => ("INVALID_REQUEST", "请求格式不对"),
    };
    let original_headers = response.headers().clone();
    let mut replacement = (status, Json(json!({"error":{"code":code,"message":message}}))).into_response();
    for name in [header::ALLOW, header::RETRY_AFTER] {
        if let Some(value) = original_headers.get(&name) {
            replacement.headers_mut().insert(name, value.clone());
        }
    }
    replacement.extensions_mut().insert(Written);
    replacement
}

fn no_such_node() -> Response {
    answer(StatusCode::NOT_FOUND, "节点不存在，可能已被删除")
}

/// The last step of every response. An error the hub composed passes as it is;
/// any other -- an extractor's rejection, the body limit's 413, a bare status --
/// would carry axum's English wording or nothing, and is given a fixed text for
/// its status instead.
pub async fn plain_errors(response: Response) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error())
        || response.extensions().get::<Written>().is_some()
    {
        return response;
    }
    let text = match status {
        StatusCode::UNAUTHORIZED => "登录已失效，请重新登录",
        StatusCode::NOT_FOUND => "请求的内容不存在",
        StatusCode::PAYLOAD_TOO_LARGE => "提交的内容过大",
        s if s.is_server_error() => INTERNAL,
        _ => "请求格式不对",
    };
    // Only the body is replaced: a 405 keeps its `Allow`, for one.
    let (mut parts, _) = response.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/plain; charset=utf-8"));
    Response::from_parts(parts, text.into())
}

mod resources;
pub(super) use resources::*;
mod live;
pub(super) use live::*;
mod provisioning;
pub(super) use provisioning::*;
mod portal;
pub(super) use portal::*;
mod nodes;
pub(super) use nodes::*;
mod storage;
pub(super) use storage::*;
mod themes;
pub(super) use themes::*;
mod settings;
pub(super) use settings::*;

#[cfg(test)]
mod tests;

/// Whether a page other than the panel sent this request. `SameSite=Lax` holds
/// a cookie back from other *sites*, but a sibling subdomain of the hub's
/// registrable domain is the same site: its script may send a request without a
/// body (`no-cors`, no preflight) and the cookie goes along, which suffices for
/// a token reset or a vacuum. Browsers name where a request came from in
/// `Sec-Fetch-Site`; only `same-origin` is the panel itself. The header is
/// absent from a client that is not a browser, from a browser too old to send
/// it, and from every browser on a plain-http address other than loopback; the
/// last two are not told apart from the panel.
fn from_other_origin(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-site").is_some_and(|site| site != "same-origin")
}

pub(super) fn cross_site_write(method: &Method, headers: &HeaderMap) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) && from_other_origin(headers)
}

/// Refuses writes from another origin before any handler sees them, so a route
/// added later is covered without remembering to ask.
pub async fn same_origin_writes(request: Request, next: Next) -> Response {
    if cross_site_write(request.method(), request.headers()) {
        return answer(StatusCode::FORBIDDEN, "请求来自其他网站，已拒绝");
    }
    next.run(request).await
}

mod realm;
pub(super) use realm::*;
