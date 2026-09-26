//! 旧的节点分享链接使用 ProxyNode UUID，身份切换后不再提供。

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;

use crate::api::{self, Admin};
use crate::Shared;

pub async fn share_proxy_node(_: Admin, _: State<Shared>, _: Path<i64>) -> Response {
    no_store(api::answer(StatusCode::GONE, "节点分享链接已停用，请在用户管理中创建代理用户并授权节点"))
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
