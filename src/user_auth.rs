//! 普通代理用户的登录 session；与管理员 monitor_session 完全分离。

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, Admin};
use crate::auth;
use crate::{App, Shared};

pub const USER_COOKIE: &str = "monitor_user_session";
const LOGIN_SESSION_DAYS: i64 = 14;
const IMPERSONATION_SESSION_SECONDS: i64 = 60 * 60;
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserSessionKind {
    Login,
    Impersonation,
}

impl UserSessionKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "login" => Some(Self::Login),
            "impersonation" => Some(Self::Impersonation),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Impersonation => "impersonation",
        }
    }
}

/// 用户中心 handler 只能拿到一个有效的 proxy_user 身份。
pub struct ProxyUserAuth {
    pub user_id: i64,
    pub session_kind: UserSessionKind,
}

impl FromRequestParts<Shared> for ProxyUserAuth {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let token = auth::cookie_value(&parts.headers, USER_COOKIE);
        let Some(token) = token else {
            return Err(api::answer(StatusCode::UNAUTHORIZED, "用户登录已失效，请重新登录"));
        };
        let session = match app.db.proxy_user_session(&auth::sha256(&token), crate::hub_time::now_timestamp())
        {
            Ok(session) => session,
            Err(error) => return Err(api::fail(error)),
        };
        match session.and_then(|(user_id, kind)| UserSessionKind::parse(&kind).map(|kind| (user_id, kind))) {
            Some((user_id, session_kind)) => Ok(Self { user_id, session_kind }),
            None => Err(api::answer(StatusCode::UNAUTHORIZED, "用户登录已失效，请重新登录")),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    username: String,
    password: String,
}

fn dummy_hash() -> Option<&'static str> {
    static HASH: OnceLock<Option<String>> = OnceLock::new();
    HASH.get_or_init(|| auth::hash_password("monitor-user-login-dummy-secret").ok()).as_deref()
}

fn session_cookie(app: &App, headers: &HeaderMap, user_id: i64, ttl: i64) -> anyhow::Result<Option<String>> {
    let now = crate::hub_time::now_timestamp();
    let token = auth::random_token();
    if !app.db.create_proxy_user_impersonation_session(&auth::sha256(&token), user_id, now, now + ttl)? {
        return Ok(None);
    }
    Ok(Some(auth::set_cookie(USER_COOKIE, &token, ttl, app.secure_cookies(headers))))
}

fn login_session_cookie(
    app: &App,
    headers: &HeaderMap,
    user_id: i64,
    is_system: bool,
    expected_password_hash: &str,
) -> anyhow::Result<Option<String>> {
    let now = crate::hub_time::now_timestamp();
    let token = auth::random_token();
    let ttl = LOGIN_SESSION_DAYS * 86_400;
    if !app.db.create_proxy_user_login_session(
        &auth::sha256(&token),
        user_id,
        is_system,
        expected_password_hash,
        now,
        now + ttl,
    )? {
        return Ok(None);
    }
    Ok(Some(auth::set_cookie(USER_COOKIE, &token, ttl, app.secure_cookies(headers))))
}

pub async fn login(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Response {
    let ip = auth::client_ip(&headers, peer.ip());
    if app.user_throttle.locked(ip) {
        return no_store(api::answer(StatusCode::TOO_MANY_REQUESTS, "尝试次数过多，稍后再试"));
    }
    let Ok(_permit) = auth::PASSWORD_GATE.try_acquire() else {
        return no_store(api::answer(StatusCode::TOO_MANY_REQUESTS, "尝试次数过多，稍后再试"));
    };
    let Ok(Json(body)) = body else {
        app.user_throttle.record_failure(ip);
        return no_store(api::answer(StatusCode::BAD_REQUEST, "用户登录请求格式不正确"));
    };
    if body.username.trim().is_empty() || body.password.len() > MAX_PASSWORD_BYTES {
        app.user_throttle.record_failure(ip);
        return no_store(api::answer(StatusCode::UNAUTHORIZED, "用户名或密码错误"));
    }

    let credential = match app.db.proxy_user_login_credential(body.username.trim()) {
        Ok(credential) => credential,
        Err(error) => return no_store(api::fail(error)),
    };
    let stored_hash = match credential.as_ref() {
        Some((_, true, _)) => app.db.get("admin_password_hash"),
        Some((_, false, password_hash)) => password_hash.clone(),
        None => None,
    };
    let hash_to_check = stored_hash.as_deref().or(dummy_hash());
    let password_matches = hash_to_check.is_some_and(|hash| auth::verify_password(&body.password, hash));
    let identity = match (credential.as_ref(), password_matches) {
        (Some((id, false, Some(_))), true) => Some((*id, false)),
        // 系统用户唯一凭据是 Monitor 管理员密码；绝不因此签发管理员 session。
        (Some((id, true, _)), true) if stored_hash.is_some() => Some((*id, true)),
        _ => None,
    };
    let Some((user_id, is_system)) = identity else {
        app.user_throttle.record_failure(ip);
        return no_store(api::answer(StatusCode::UNAUTHORIZED, "用户名或密码错误"));
    };
    let Some(expected_password_hash) = stored_hash.as_deref() else {
        app.user_throttle.record_failure(ip);
        return no_store(api::answer(StatusCode::UNAUTHORIZED, "用户名或密码错误"));
    };
    match login_session_cookie(&app, &headers, user_id, is_system, expected_password_hash) {
        Ok(Some(cookie)) => {
            app.user_throttle.clear(ip);
            no_store(auth::with_cookies(Json(json!({ "ok": true })), [cookie]))
        }
        Ok(None) => {
            app.user_throttle.record_failure(ip);
            no_store(api::answer(StatusCode::UNAUTHORIZED, "用户名或密码错误"))
        }
        Err(error) => no_store(api::fail(error)),
    }
}

pub async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::cookie_value(&headers, USER_COOKIE) {
        if let Err(error) = app.db.drop_proxy_user_session(&auth::sha256(&token)) {
            return no_store(api::fail(error));
        }
    }
    no_store(auth::with_cookies(
        Json(json!({ "ok": true })),
        [auth::set_cookie(USER_COOKIE, "", 0, app.secure_cookies(&headers))],
    ))
}

pub async fn me(user: ProxyUserAuth, State(app): State<Shared>) -> Response {
    match app.db.proxy_user(user.user_id) {
        Ok(Some(profile)) => no_store(
            Json(json!({
                "user": {
                    "id": profile.id,
                    "username": profile.name,
                    "is_system": profile.is_system
                },
                "session_kind": user.session_kind.label()
            }))
            .into_response(),
        ),
        Ok(None) => no_store(api::answer(StatusCode::UNAUTHORIZED, "用户登录已失效，请重新登录")),
        Err(error) => no_store(api::fail(error)),
    }
}

/// 管理员在新用户中心打开时，创建一个短期且权限仍为普通用户的 session。
pub async fn impersonate(
    _: Admin,
    State(app): State<Shared>,
    Path(user_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if user_id <= 0 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "代理用户 ID 无效"));
    }
    match app.db.proxy_user(user_id) {
        Ok(Some(_)) => {}
        Ok(None) => return no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
        Err(error) => return no_store(api::fail(error)),
    }
    match session_cookie(&app, &headers, user_id, IMPERSONATION_SESSION_SECONDS) {
        Ok(Some(cookie)) => {
            no_store(auth::with_cookies(Json(json!({ "ok": true, "url": "/user" })), [cookie]))
        }
        Ok(None) => no_store(api::answer(StatusCode::NOT_FOUND, "代理用户不存在")),
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

    use crate::api::Admin;
    use crate::auth;
    use crate::db::{Db, ProxyUserCreateSettings, ProxyUserLimits};
    use crate::proxy_provision;
    use crate::App;
    use axum::body::to_bytes;
    use axum::extract::FromRequestParts;
    use axum::http::request::Request;
    use std::sync::Arc;

    static LOGIN_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn app() -> Shared {
        Arc::new(App::for_test(Db::open(":memory:").expect("test database")))
    }

    fn add_user(app: &App, name: &str, password: Option<&str>) -> i64 {
        let hash = password.map(|password| auth::hash_password(password).expect("password hash"));
        let (user, _) = if let Some(hash) = hash.as_deref() {
            app.db
                .create_proxy_user_with_password(
                    name,
                    &proxy_provision::uuid_v4(),
                    true,
                    "",
                    &[],
                    ProxyUserCreateSettings { limits: ProxyUserLimits::default(), password_hash: Some(hash) },
                )
                .expect("create user")
        } else {
            app.db
                .create_proxy_user_with_limits(
                    name,
                    &proxy_provision::uuid_v4(),
                    true,
                    "",
                    &[],
                    ProxyUserLimits::default(),
                )
                .expect("create user")
        };
        user.id
    }

    fn login_request(username: &str, password: &str) -> Result<Json<LoginRequest>, JsonRejection> {
        Ok(Json(LoginRequest { username: username.into(), password: password.into() }))
    }

    async fn body_json(response: Response) -> serde_json::Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.expect("response body"))
            .expect("response JSON")
    }

    fn cookie(response: &Response) -> String {
        response
            .headers()
            .get(header::SET_COOKIE)
            .expect("user session cookie")
            .to_str()
            .expect("cookie header")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned()
    }

    #[tokio::test]
    async fn ordinary_user_login_issues_only_a_user_session_and_me_uses_that_identity() {
        let _serial = LOGIN_TEST_LOCK.lock().await;
        let app = app();
        let user_id = add_user(&app, "ordinary", Some("a-secure-user-password"));
        let response = login(
            State(app.clone()),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))),
            HeaderMap::new(),
            login_request("ordinary", "a-secure-user-password"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let pair = cookie(&response);
        assert!(pair.starts_with("monitor_user_session="));
        assert!(!pair.starts_with("monitor_session="));
        let token = pair.split_once('=').expect("cookie token").1.to_owned();
        let mut request = Request::new(axum::body::Body::empty());
        request.headers_mut().insert(header::COOKIE, pair.parse().expect("cookie header"));
        let (mut parts, _) = request.into_parts();
        let identity = ProxyUserAuth::from_request_parts(&mut parts, &app).await.expect("user identity");
        assert_eq!(identity.user_id, user_id);
        assert_eq!(identity.session_kind, UserSessionKind::Login);
        assert!(
            !app.db.session_valid(&auth::sha256(&token)),
            "a user login must not create an admin session"
        );
        let me = body_json(me(identity, State(app.clone())).await).await;
        assert_eq!(me["user"]["username"], "ordinary");
        assert_eq!(me["session_kind"], "login");
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_username_are_rejected() {
        let _serial = LOGIN_TEST_LOCK.lock().await;
        let app = app();
        add_user(&app, "ordinary", Some("a-secure-user-password"));
        for (username, password) in [("ordinary", "wrong-password"), ("missing", "wrong-password")] {
            let response = login(
                State(app.clone()),
                ConnectInfo(SocketAddr::from(([127, 0, 0, 2], 4000))),
                HeaderMap::new(),
                login_request(username, password),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(response.headers().get(header::SET_COOKIE).is_none());
        }
        for _ in 0..3 {
            let response = login(
                State(app.clone()),
                ConnectInfo(SocketAddr::from(([127, 0, 0, 2], 4000))),
                HeaderMap::new(),
                login_request("missing", "wrong-password"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        assert!(app.user_throttle.locked("127.0.0.2".parse().unwrap()));
        assert!(!app.throttle.locked("127.0.0.2".parse().unwrap()), "用户密码失败不应锁定管理员登录");
        let limited = login(
            State(app),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 2], 4000))),
            HeaderMap::new(),
            login_request("ordinary", "a-secure-user-password"),
        )
        .await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn disabled_expired_or_over_quota_users_can_still_enter_the_user_center() {
        let _serial = LOGIN_TEST_LOCK.lock().await;
        let app = app();
        let id = add_user(&app, "inactive", Some("a-secure-user-password"));
        app.db
            .update_proxy_user_settings(
                id,
                "inactive",
                false,
                "",
                &[],
                ProxyUserLimits { traffic_limit_bytes: 1, traffic_reset_day: 1, expire_at: Some(1) },
            )
            .expect("set inactive state");
        app.db.set_proxy_user_usage_for_test(id, 1, 0).expect("set over-quota usage");

        let response = login(
            State(app),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 4], 4000))),
            HeaderMap::new(),
            login_request("inactive", "a-secure-user-password"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_password_login_gets_only_the_system_proxy_user_identity() {
        let _serial = LOGIN_TEST_LOCK.lock().await;
        let app = app();
        let password_hash = auth::hash_password("existing-admin-password").expect("admin hash");
        app.db.set("admin_password_hash", &password_hash).expect("save admin hash");
        let admin_id =
            app.db.proxy_users().expect("users").into_iter().find(|user| user.is_system).unwrap().id;
        let response = login(
            State(app.clone()),
            ConnectInfo(SocketAddr::from(([127, 0, 0, 3], 4000))),
            HeaderMap::new(),
            login_request("admin", "existing-admin-password"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let pair = cookie(&response);
        let token = pair.split_once('=').expect("cookie token").1;
        let request =
            Request::builder().header(header::COOKIE, pair.as_str()).body(axum::body::Body::empty()).unwrap();
        let (mut parts, _) = request.into_parts();
        let identity =
            ProxyUserAuth::from_request_parts(&mut parts, &app).await.expect("admin as proxy user");
        assert_eq!(identity.user_id, admin_id);
        assert_eq!(identity.session_kind, UserSessionKind::Login);
        assert!(!app.db.session_valid(&auth::sha256(token)));
        assert_eq!(app.db.proxy_user_session(&auth::sha256(token), i64::MAX).expect("session query"), None);
    }

    #[tokio::test]
    async fn user_session_cannot_satisfy_the_admin_extractor() {
        let app = app();
        let id = add_user(&app, "ordinary", Some("a-secure-user-password"));
        let token = auth::random_token();
        let password_hash = app.db.proxy_user_login_credential("ordinary").unwrap().unwrap().2.unwrap();
        app.db
            .create_proxy_user_login_session(&auth::sha256(&token), id, false, &password_hash, 1, i64::MAX)
            .expect("create user session");
        let request = Request::builder()
            .header(header::COOKIE, format!("{USER_COOKIE}={token}"))
            .body(axum::body::Body::empty())
            .expect("request");
        let (mut parts, _) = request.into_parts();
        let rejected = Admin::from_request_parts(&mut parts, &app)
            .await
            .err()
            .expect("user session has no admin authority");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_session_cannot_satisfy_the_user_extractor() {
        let app = app();
        let token = auth::random_token();
        app.db.create_session(&auth::sha256(&token), i64::MAX).unwrap();
        let request = Request::builder()
            .header(header::COOKIE, format!("{}={token}", auth::COOKIE))
            .body(axum::body::Body::empty())
            .expect("request");
        let (mut parts, _) = request.into_parts();
        let rejected =
            ProxyUserAuth::from_request_parts(&mut parts, &app).await.err().expect("no user identity");
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_can_impersonate_but_a_user_session_cannot_call_the_admin_route() {
        let app = app();
        let id = add_user(&app, "ordinary", None);
        let before = crate::hub_time::now_timestamp();
        let response = impersonate(Admin, State(app.clone()), Path(id), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let pair = cookie(&response);
        assert_eq!(body_json(response).await["url"], "/user");
        let token = pair.split_once('=').expect("cookie token").1;
        assert!(pair.starts_with("monitor_user_session="));
        assert_eq!(
            app.db
                .proxy_user_session(&auth::sha256(token), crate::hub_time::now_timestamp())
                .expect("session"),
            Some((id, "impersonation".to_owned()))
        );
        assert!(app
            .db
            .proxy_user_session(&auth::sha256(token), before + IMPERSONATION_SESSION_SECONDS - 1)
            .expect("session before expiry")
            .is_some());
        assert!(app
            .db
            .proxy_user_session(&auth::sha256(token), before + IMPERSONATION_SESSION_SECONDS + 1)
            .expect("session after expiry")
            .is_none());

        let request = Request::builder()
            .header(header::COOKIE, pair.as_str())
            .body(axum::body::Body::empty())
            .expect("request");
        let (mut parts, _) = request.into_parts();
        assert!(Admin::from_request_parts(&mut parts, &app).await.is_err());
    }

    #[tokio::test]
    async fn logout_revokes_only_the_user_session_and_clears_its_cookie() {
        let app = app();
        let id = add_user(&app, "ordinary", None);
        let token = auth::random_token();
        app.db
            .create_proxy_user_impersonation_session(
                &auth::sha256(&token),
                id,
                crate::hub_time::now_timestamp(),
                crate::hub_time::now_timestamp() + 60,
            )
            .expect("create session");
        let headers =
            HeaderMap::from_iter([(header::COOKIE, format!("{USER_COOKIE}={token}").parse().unwrap())]);
        let response = logout(State(app.clone()), headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            app.db.proxy_user_session(&auth::sha256(&token), crate::hub_time::now_timestamp()).unwrap(),
            None
        );
        assert!(response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    }
}
