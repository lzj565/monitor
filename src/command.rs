//! Hub 与 agent 之间的内置命令请求、响应和管理员查询接口。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::debug;

use crate::api::{self, Admin};
use crate::auth::{random_token, sha256};
use crate::{App, Shared};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_COMMAND_TIMEOUT: Duration = Duration::from_secs(35);
const CONFIG_CHECK_TIMEOUT: Duration = Duration::from_secs(25);
const CONFIG_APPLY_TIMEOUT: Duration = Duration::from_secs(110);
const RESULT_RETENTION: Duration = Duration::from_secs(10 * 60);
const CONFIG_RESULT_RETENTION: Duration = Duration::from_secs(60);
const MAX_CONFIG_BYTES: usize = 32 * 1024;
const AGENT_STATUS_METHOD: &str = "agent.status";
const SINGBOX_STATUS_METHOD: &str = "singbox.status";
const SINGBOX_CONFIG_GET_METHOD: &str = "singbox.config.get";
const SINGBOX_CONFIG_CHECK_METHOD: &str = "singbox.config.check";
const SINGBOX_CONFIG_APPLY_METHOD: &str = "singbox.config.apply";
const SINGBOX_CONTROL_METHODS: [&str; 4] =
    ["singbox.start", "singbox.stop", "singbox.restart", "singbox.reload"];
const REMOTE_TIMEOUT_CODE: i64 = -32001;

fn is_control_method(method: &str) -> bool {
    SINGBOX_CONTROL_METHODS.contains(&method)
}

fn is_config_method(method: &str) -> bool {
    method == SINGBOX_CONFIG_GET_METHOD
        || method == SINGBOX_CONFIG_CHECK_METHOD
        || method == SINGBOX_CONFIG_APPLY_METHOD
}

fn is_supported_method(method: &str) -> bool {
    method == AGENT_STATUS_METHOD
        || method == SINGBOX_STATUS_METHOD
        || is_config_method(method)
        || is_control_method(method)
}

fn result_retention(method: &str) -> Duration {
    if method == SINGBOX_CONFIG_GET_METHOD {
        CONFIG_RESULT_RETENTION
    } else {
        RESULT_RETENTION
    }
}

fn command_timeout(method: &str) -> Duration {
    if method == SINGBOX_CONFIG_APPLY_METHOD {
        CONFIG_APPLY_TIMEOUT
    } else if method == SINGBOX_CONFIG_CHECK_METHOD {
        CONFIG_CHECK_TIMEOUT
    } else if is_control_method(method) {
        CONTROL_COMMAND_TIMEOUT
    } else {
        COMMAND_TIMEOUT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Succeeded,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Serialize)]
pub struct View {
    command_id: String,
    method: String,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

struct Record {
    node_id: i64,
    session: u64,
    method: String,
    status: Status,
    result: Option<Value>,
    error: Option<Value>,
    expires_at: Option<Instant>,
}

#[derive(Default)]
pub struct Registry {
    records: Mutex<HashMap<String, Record>>,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Record>> {
        self.records.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn purge(records: &mut HashMap<String, Record>, now: Instant) {
        records.retain(|_, record| record.expires_at.is_none_or(|expiry| expiry > now));
    }

    fn insert(&self, id: String, node_id: i64, session: u64, method: String) {
        let mut records = self.lock();
        Self::purge(&mut records, Instant::now());
        records.insert(
            id,
            Record {
                node_id,
                session,
                method,
                status: Status::Pending,
                result: None,
                error: None,
                expires_at: None,
            },
        );
    }

    fn remove(&self, id: &str) {
        self.lock().remove(id);
    }

    fn purge_expired(&self) {
        Self::purge(&mut self.lock(), Instant::now());
    }

    fn finish(&self, id: &str, node_id: i64, session: u64, result: Result<Value, Value>) -> bool {
        let now = Instant::now();
        let mut records = self.lock();
        Self::purge(&mut records, now);
        let Some(record) = records.get_mut(id) else { return false };
        if record.node_id != node_id || record.session != session || record.status != Status::Pending {
            return false;
        }
        let result = if record.method == SINGBOX_CONFIG_GET_METHOD {
            result.and_then(|mut value| {
                let content = value.get("content").and_then(Value::as_str).ok_or_else(
                    || json!({"code": -32603, "message": "agent 返回的 sing-box 配置格式不正确"}),
                )?;
                value["sha256"] = json!(sha256(content));
                Ok(value)
            })
        } else {
            result
        };
        match result {
            Ok(result) => {
                record.status = Status::Succeeded;
                record.result = Some(result);
            }
            Err(error) if error.get("code").and_then(Value::as_i64) == Some(REMOTE_TIMEOUT_CODE) => {
                record.status = Status::OutcomeUnknown;
                record.error = Some(json!({
                    "reason": "timeout",
                    "message": if is_config_content_method(&record.method) {
                        error.get("message").and_then(Value::as_str).unwrap_or("sing-box 配置应用结果未知")
                    } else {
                        "Agent 本地执行超时，服务操作可能已生效"
                    }
                }));
            }
            Err(error) => {
                record.status = Status::Failed;
                record.error = Some(error);
            }
        }
        record.expires_at = Some(now + result_retention(&record.method));
        true
    }

    fn outcome_unknown(&self, id: &str, session: u64, reason: &str) {
        let now = Instant::now();
        let mut records = self.lock();
        Self::purge(&mut records, now);
        let Some(record) = records.get_mut(id) else { return };
        if record.session != session || record.status != Status::Pending {
            return;
        }
        record.status = Status::OutcomeUnknown;
        record.error = Some(json!({
            "reason": reason,
            "message": if reason == "timeout" { "命令等待响应超时，agent 可能已经执行" } else { "agent 连接中断，命令执行结果未知" }
        }));
        record.expires_at = Some(now + result_retention(&record.method));
    }

    fn disconnect(&self, node_id: i64, session: u64) {
        let now = Instant::now();
        let mut records = self.lock();
        Self::purge(&mut records, now);
        for record in records.values_mut() {
            if record.node_id == node_id && record.session == session && record.status == Status::Pending {
                record.status = Status::OutcomeUnknown;
                record.error = Some(json!({
                    "reason": "disconnected",
                    "message": "agent 连接中断，命令执行结果未知"
                }));
                record.expires_at = Some(now + result_retention(&record.method));
            }
        }
    }

    pub fn complete(&self, id: &str, node_id: i64, session: u64, result: Result<Value, Value>) -> bool {
        self.finish(id, node_id, session, result)
    }

    fn get(&self, id: &str, node_id: i64) -> Option<View> {
        let now = Instant::now();
        let mut records = self.lock();
        Self::purge(&mut records, now);
        let record = records.get(id).filter(|record| record.node_id == node_id)?;
        Some(View {
            command_id: id.to_owned(),
            method: record.method.clone(),
            status: record.status,
            result: record.result.clone(),
            error: record.error.clone(),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    method: String,
    #[serde(default = "empty_params")]
    params: Value,
}

fn empty_params() -> Value {
    json!({})
}

pub async fn submit(
    _: Admin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: Result<Json<Request>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return api::answer(StatusCode::BAD_REQUEST, "命令请求格式不正确");
    };
    if !is_supported_method(&request.method) {
        return api::answer(StatusCode::BAD_REQUEST, "未注册的命令方法");
    }
    if is_config_content_method(&request.method) {
        let Some(values) = request.params.as_object() else {
            return api::answer(StatusCode::BAD_REQUEST, "配置命令参数必须是对象");
        };
        if values.len() != 1 || !values.contains_key("content") {
            return api::answer(StatusCode::BAD_REQUEST, "配置命令只接受 content 参数");
        }
        let Some(content) = values.get("content").and_then(Value::as_str) else {
            return api::answer(StatusCode::BAD_REQUEST, "配置命令 content 必须是字符串");
        };
        if content.len() > MAX_CONFIG_BYTES {
            return api::answer(StatusCode::PAYLOAD_TOO_LARGE, "sing-box 配置超过 32 KiB 限制");
        }
    } else if request.params != json!({}) {
        return api::answer(StatusCode::BAD_REQUEST, format!("{} 不接收参数", request.method));
    }
    match app.db.node(node_id) {
        Ok(Some(_)) => {}
        Ok(None) => return api::answer(StatusCode::NOT_FOUND, "节点不存在，可能已被删除"),
        Err(e) => return api::fail(e),
    }

    let (session, sender) = {
        let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
        let Some(agent) = agents.get(&node_id) else {
            return api::answer(StatusCode::CONFLICT, "节点当前离线");
        };
        if !agent.capabilities.contains(&request.method) {
            return api::answer(StatusCode::CONFLICT, "当前 agent 不支持该命令方法");
        }
        (agent.session, agent.tx.clone())
    };

    let id = random_token()[..24].to_owned();
    let timeout = command_timeout(&request.method);
    let method = request.method;
    let message = request_message(&id, &method, request.params);
    app.commands.insert(id.clone(), node_id, session, method.to_owned());
    match sender.try_send(message) {
        Ok(()) => {
            let app_for_timeout = app.clone();
            let timeout_id = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                app_for_timeout.commands.outcome_unknown(&timeout_id, session, "timeout");
                if method == SINGBOX_CONFIG_GET_METHOD {
                    tokio::time::sleep(CONFIG_RESULT_RETENTION).await;
                    app_for_timeout.commands.purge_expired();
                }
            });
            no_store(
                (StatusCode::ACCEPTED, Json(json!({"command_id": id, "status": Status::Pending})))
                    .into_response(),
            )
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            app.commands.remove(&id);
            api::answer(StatusCode::SERVICE_UNAVAILABLE, "节点命令队列已满")
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            app.commands.remove(&id);
            api::answer(StatusCode::CONFLICT, "节点连接已断开")
        }
    }
}

pub async fn status(
    _: Admin,
    State(app): State<Shared>,
    Path((node_id, command_id)): Path<(i64, String)>,
) -> Response {
    match app.commands.get(&command_id, node_id) {
        Some(view) => no_store(Json(view).into_response()),
        None => no_store(api::answer(StatusCode::NOT_FOUND, "命令不存在或结果已过期")),
    }
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn is_config_content_method(method: &str) -> bool {
    method == SINGBOX_CONFIG_CHECK_METHOD || method == SINGBOX_CONFIG_APPLY_METHOD
}

/// 清理指定连接中仍未收到结果的命令，断连后的执行结果不能再被确认。
pub fn disconnect(app: &App, node_id: i64, session: u64) {
    app.commands.disconnect(node_id, session);
}

pub fn complete(app: &App, id: &str, node_id: i64, session: u64, result: Result<Value, Value>) -> bool {
    let accepted = app.commands.complete(id, node_id, session, result);
    if !accepted {
        debug!("node {node_id}: ignored unknown, late, or mismatched command response {id}");
    }
    accepted
}

pub fn request_message(id: &str, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_ws::Agent;
    use crate::db::{Db, Node};
    use std::sync::Arc;

    fn app_node() -> (Shared, i64) {
        let app = Arc::new(App::for_test(Db::open(":memory:").unwrap()));
        let id = app
            .db
            .create_node(&Node { name: "command-node".into(), ..Node::default() }, "node-token")
            .unwrap();
        (app, id)
    }

    async fn response_json(response: Response) -> Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await;
        serde_json::from_slice(&body.unwrap()).unwrap()
    }

    #[test]
    fn command_results_are_bound_to_the_node_and_connection_that_requested_them() {
        let registry = Registry::default();
        registry.insert("id".into(), 7, 11, AGENT_STATUS_METHOD.into());
        assert!(!registry.complete("id", 8, 11, Ok(json!({"ok": true}))));
        assert!(!registry.complete("id", 7, 12, Ok(json!({"ok": true}))));
        assert!(registry.complete("id", 7, 11, Ok(json!({"agent_version": "1.0"}))));
        assert!(!registry.complete("id", 7, 11, Ok(json!({"late": true}))));
        let result = registry.get("id", 7).unwrap();
        assert_eq!(result.status, Status::Succeeded);
        assert_eq!(result.result.unwrap()["agent_version"], "1.0");
        assert!(registry.get("id", 8).is_none());
    }

    #[test]
    fn a_disconnected_command_has_an_unknown_outcome() {
        let registry = Registry::default();
        registry.insert("id".into(), 7, 11, AGENT_STATUS_METHOD.into());
        registry.disconnect(7, 11);
        let result = registry.get("id", 7).unwrap();
        assert_eq!(result.status, Status::OutcomeUnknown);
        assert_eq!(result.error.unwrap()["reason"], "disconnected");
    }

    #[test]
    fn an_agent_error_is_returned_as_a_failed_command() {
        let registry = Registry::default();
        registry.insert("id".into(), 7, 11, AGENT_STATUS_METHOD.into());
        assert!(registry.complete("id", 7, 11, Err(json!({"code": -32603, "message": "failed"}))));
        let result = registry.get("id", 7).unwrap();
        assert_eq!(result.status, Status::Failed);
        assert_eq!(result.error.unwrap()["code"], -32603);
    }

    #[test]
    fn an_agent_control_timeout_has_an_unknown_outcome() {
        let registry = Registry::default();
        registry.insert("id".into(), 7, 11, "singbox.restart".into());
        assert!(registry.complete(
            "id",
            7,
            11,
            Err(json!({"code": REMOTE_TIMEOUT_CODE, "message": "结果可能已生效"}))
        ));
        let result = registry.get("id", 7).unwrap();
        assert_eq!(result.status, Status::OutcomeUnknown);
        assert_eq!(result.error.unwrap()["reason"], "timeout");
        assert!(!registry.complete("id", 7, 11, Ok(json!({"late": true}))));
    }

    #[test]
    fn config_apply_unknown_outcome_keeps_the_rollback_backup_location() {
        let registry = Registry::default();
        registry.insert("apply-id".into(), 7, 11, SINGBOX_CONFIG_APPLY_METHOD.into());
        let message = "旧配置恢复失败；备份保留于 /etc/sing-box/.config.json.backup-1-2";
        assert!(registry.complete(
            "apply-id",
            7,
            11,
            Err(json!({"code": REMOTE_TIMEOUT_CODE, "message": message}))
        ));
        let result = registry.get("apply-id", 7).unwrap();
        assert_eq!(result.status, Status::OutcomeUnknown);
        assert_eq!(result.error.unwrap()["message"], message);
    }

    #[test]
    fn requests_use_json_rpc_ids_methods_and_params() {
        let parsed: Value =
            serde_json::from_str(&request_message("id", AGENT_STATUS_METHOD, json!({}))).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], "id");
        assert_eq!(parsed["method"], AGENT_STATUS_METHOD);
        assert_eq!(parsed["params"], json!({}));
    }

    #[tokio::test]
    async fn administrator_can_submit_and_poll_a_single_node_command() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(11, tx);
        agent.capabilities.insert(AGENT_STATUS_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);

        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: AGENT_STATUS_METHOD.into(), params: json!({}) })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let created = response_json(response).await;
        let id = created["command_id"].as_str().unwrap();
        assert_eq!(created["status"], "pending");

        let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(sent["id"], id);
        assert_eq!(sent["method"], AGENT_STATUS_METHOD);
        assert_eq!(app.commands.get(id, node_id).unwrap().status, Status::Pending);

        assert!(app.commands.complete(id, node_id, 11, Ok(json!({"agent_version": "1.2.3"}))));
        let response = status(Admin, State(app.clone()), Path((node_id, id.to_owned()))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let completed = response_json(response).await;
        assert_eq!(completed["status"], "succeeded");
        assert_eq!(completed["result"]["agent_version"], "1.2.3");
    }

    #[tokio::test]
    async fn administrator_can_submit_and_poll_singbox_status() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(15, tx);
        agent.capabilities.insert(SINGBOX_STATUS_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);

        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: SINGBOX_STATUS_METHOD.into(), params: json!({}) })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let created = response_json(response).await;
        let id = created["command_id"].as_str().unwrap();
        let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(sent["id"], id);
        assert_eq!(sent["method"], SINGBOX_STATUS_METHOD);
        assert_eq!(sent["params"], json!({}));

        let result = json!({
            "installed": true,
            "version": "1.12.0",
            "service_exists": true,
            "running": true,
            "config_exists": true,
            "config_path": "/etc/sing-box/config.json"
        });
        assert!(app.commands.complete(id, node_id, 15, Ok(result.clone())));
        let response = status(Admin, State(app.clone()), Path((node_id, id.to_owned()))).await;
        let completed = response_json(response).await;
        assert_eq!(completed["status"], "succeeded");
        assert_eq!(completed["result"], result);
    }

    #[tokio::test]
    async fn config_get_uses_the_existing_command_channel_and_no_store_response() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(18, tx);
        agent.capabilities.insert(SINGBOX_CONFIG_GET_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);

        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: SINGBOX_CONFIG_GET_METHOD.into(), params: json!({}) })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let created = response_json(response).await;
        let id = created["command_id"].as_str().unwrap();
        let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(sent["id"], id);
        assert_eq!(sent["method"], SINGBOX_CONFIG_GET_METHOD);
        assert_eq!(sent["params"], json!({}));

        let result = json!({"content": "{}", "size_bytes": 2, "modified_at": null, "config_path": "/etc/sing-box/config.json"});
        assert!(app.commands.complete(id, node_id, 18, Ok(result.clone())));
        let response = status(Admin, State(app.clone()), Path((node_id, id.to_owned()))).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let completed = response_json(response).await;
        assert_eq!(completed["result"]["content"], result["content"]);
        assert_eq!(
            completed["result"]["sha256"],
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        let remaining = app.commands.lock()[id].expires_at.unwrap().duration_since(Instant::now());
        assert!(remaining <= CONFIG_RESULT_RETENTION && remaining >= Duration::from_secs(55));
    }

    #[tokio::test]
    async fn config_check_and_apply_send_only_the_content_parameter() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(19, tx);
        agent.capabilities.insert(SINGBOX_CONFIG_CHECK_METHOD.into());
        agent.capabilities.insert(SINGBOX_CONFIG_APPLY_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);

        for (method, result) in [
            (SINGBOX_CONFIG_CHECK_METHOD, json!({"valid": true, "size_bytes": 2})),
            (
                SINGBOX_CONFIG_APPLY_METHOD,
                json!({"config_path": "/etc/sing-box/config.json", "size_bytes": 2, "running": true}),
            ),
        ] {
            let response = submit(
                Admin,
                State(app.clone()),
                Path(node_id),
                Ok(Json(Request { method: method.into(), params: json!({"content": "{}"}) })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let created = response_json(response).await;
            let id = created["command_id"].as_str().unwrap();
            let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            assert_eq!(sent["id"], id);
            assert_eq!(sent["method"], method);
            assert_eq!(sent["params"], json!({"content": "{}"}));

            assert!(app.commands.complete(id, node_id, 19, Ok(result.clone())));
            let response = status(Admin, State(app.clone()), Path((node_id, id.to_owned()))).await;
            let completed = response_json(response).await;
            assert_eq!(completed["status"], "succeeded");
            assert_eq!(completed["result"], result);
            assert!(completed["result"].get("content").is_none());
        }
        assert_eq!(command_timeout(SINGBOX_CONFIG_CHECK_METHOD), CONFIG_CHECK_TIMEOUT);
        assert_eq!(command_timeout(SINGBOX_CONFIG_APPLY_METHOD), CONFIG_APPLY_TIMEOUT);
    }

    #[tokio::test]
    async fn config_mutation_rejects_extra_params_and_oversized_content_before_queueing() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(20, tx);
        agent.capabilities.insert(SINGBOX_CONFIG_CHECK_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);

        let extra = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request {
                method: SINGBOX_CONFIG_CHECK_METHOD.into(),
                params: json!({"content": "{}", "extra": true}),
            })),
        )
        .await;
        assert_eq!(extra.status(), StatusCode::BAD_REQUEST);

        let oversized = submit(
            Admin,
            State(app),
            Path(node_id),
            Ok(Json(Request {
                method: SINGBOX_CONFIG_CHECK_METHOD.into(),
                params: json!({"content": "x".repeat(MAX_CONFIG_BYTES + 1)}),
            })),
        )
        .await;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn malformed_config_result_fails_without_exposing_content_and_expires() {
        let registry = Registry::default();
        registry.insert("config-id".into(), 7, 11, SINGBOX_CONFIG_GET_METHOD.into());
        assert!(registry.complete("config-id", 7, 11, Ok(json!({"unexpected": "secret"}))));
        let result = registry.get("config-id", 7).unwrap();
        assert_eq!(result.status, Status::Failed);
        assert!(result.result.is_none());
        assert_eq!(result.error.unwrap()["message"], "agent 返回的 sing-box 配置格式不正确");
        registry.lock().get_mut("config-id").unwrap().expires_at =
            Some(Instant::now() - Duration::from_secs(1));
        registry.purge_expired();
        assert!(registry.lock().is_empty());
    }

    #[tokio::test]
    async fn administrator_can_submit_each_fixed_singbox_control_method() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(16, tx);
        for method in SINGBOX_CONTROL_METHODS {
            agent.capabilities.insert(method.into());
        }
        app.agents.write().unwrap().insert(node_id, agent);

        for method in SINGBOX_CONTROL_METHODS {
            let response = submit(
                Admin,
                State(app.clone()),
                Path(node_id),
                Ok(Json(Request { method: method.into(), params: json!({}) })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let created = response_json(response).await;
            let sent: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            assert_eq!(sent["id"], created["command_id"]);
            assert_eq!(sent["method"], method);
            assert_eq!(sent["params"], json!({}));
        }
    }

    #[tokio::test]
    async fn offline_unsupported_and_unregistered_commands_are_rejected() {
        let (app, node_id) = app_node();
        let request = || {
            Ok::<_, JsonRejection>(Json(Request { method: AGENT_STATUS_METHOD.into(), params: json!({}) }))
        };
        let offline = submit(Admin, State(app.clone()), Path(node_id), request()).await;
        assert_eq!(offline.status(), StatusCode::CONFLICT);

        let (tx, _rx) = mpsc::channel(16);
        app.agents.write().unwrap().insert(node_id, Agent::new(12, tx));
        let unsupported = submit(Admin, State(app.clone()), Path(node_id), request()).await;
        assert_eq!(unsupported.status(), StatusCode::CONFLICT);

        let invalid_singbox_params = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: SINGBOX_STATUS_METHOD.into(), params: json!({"unexpected": true}) })),
        )
        .await;
        assert_eq!(invalid_singbox_params.status(), StatusCode::BAD_REQUEST);

        let unsupported_singbox = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: SINGBOX_STATUS_METHOD.into(), params: json!({}) })),
        )
        .await;
        assert_eq!(unsupported_singbox.status(), StatusCode::CONFLICT);

        let invalid_control_params = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: "singbox.restart".into(), params: json!({"force": true}) })),
        )
        .await;
        assert_eq!(invalid_control_params.status(), StatusCode::BAD_REQUEST);

        let unsupported_control = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: "singbox.restart".into(), params: json!({}) })),
        )
        .await;
        assert_eq!(unsupported_control.status(), StatusCode::CONFLICT);

        let unknown = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: "shell.exec".into(), params: json!({"command": "id"}) })),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_full_agent_outbound_queue_refuses_the_command_without_a_record() {
        let (app, node_id) = app_node();
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send("occupied".to_owned()).unwrap();
        let mut agent = Agent::new(14, tx);
        agent.capabilities.insert(AGENT_STATUS_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);
        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: AGENT_STATUS_METHOD.into(), params: json!({}) })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(app.commands.lock().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_commands_become_unknown_and_expired_results_disappear() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(13, tx);
        agent.capabilities.insert(AGENT_STATUS_METHOD.into());
        app.agents.write().unwrap().insert(node_id, agent);
        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: AGENT_STATUS_METHOD.into(), params: json!({}) })),
        )
        .await;
        let created = response_json(response).await;
        let id = created["command_id"].as_str().unwrap().to_owned();
        let _ = rx.recv().await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(COMMAND_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(app.commands.get(&id, node_id).unwrap().status, Status::OutcomeUnknown);

        let mut records = app.commands.lock();
        records.get_mut(&id).unwrap().expires_at = Some(Instant::now() - Duration::from_secs(1));
        drop(records);
        assert!(app.commands.get(&id, node_id).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn control_commands_wait_longer_than_status_queries() {
        let (app, node_id) = app_node();
        let (tx, mut rx) = mpsc::channel(16);
        let mut agent = Agent::new(17, tx);
        agent.capabilities.insert("singbox.restart".into());
        app.agents.write().unwrap().insert(node_id, agent);
        let response = submit(
            Admin,
            State(app.clone()),
            Path(node_id),
            Ok(Json(Request { method: "singbox.restart".into(), params: json!({}) })),
        )
        .await;
        let created = response_json(response).await;
        let id = created["command_id"].as_str().unwrap();
        let _ = rx.recv().await.unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(COMMAND_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(app.commands.get(id, node_id).unwrap().status, Status::Pending);
        tokio::time::advance(CONTROL_COMMAND_TIMEOUT - COMMAND_TIMEOUT).await;
        tokio::task::yield_now().await;
        assert_eq!(app.commands.get(id, node_id).unwrap().status, Status::OutcomeUnknown);
    }
}
