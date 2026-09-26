//! 手动触发的服务器级 ProxyNode 配置部署。

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::api::{self, Admin};
use crate::command::{self, ConfigCommandError};
use crate::db::ProxyNode;
use crate::proxy_config::{generate_singbox_config, ProxyConfigError};
#[cfg(test)]
use crate::App;
use crate::Shared;

#[derive(Debug)]
struct DeployResult {
    changed: bool,
    deployed_nodes: usize,
}

#[derive(Debug)]
enum DeployError {
    Database(anyhow::Error),
    ServerNotFound,
    ProxyNodeNotFound,
    AgentOffline,
    AgentUnsupported,
    AgentQueueFull,
    AgentDisconnected,
    Timeout,
    OutcomeUnknown,
    ConfigTooLarge,
    ConfigGetFailed,
    ConfigGetResponseInvalid,
    ConfigParseFailed,
    Generator(ProxyConfigError),
    ConfigCheckFailed,
    ConfigApplyFailed,
}

impl DeployError {
    fn last_error(&self) -> String {
        match self {
            Self::Database(_) => "hub database error".into(),
            Self::ServerNotFound => "server not found".into(),
            Self::ProxyNodeNotFound => "proxy node not found".into(),
            Self::AgentOffline => "agent offline".into(),
            Self::AgentUnsupported => "agent does not support sing-box config commands".into(),
            Self::AgentQueueFull => "agent command queue full".into(),
            Self::AgentDisconnected => "agent disconnected".into(),
            Self::Timeout => "deployment timeout".into(),
            Self::OutcomeUnknown => "deployment outcome unknown".into(),
            Self::ConfigTooLarge => "sing-box config exceeds 32 KiB limit".into(),
            Self::ConfigGetFailed => "config get failed".into(),
            Self::ConfigGetResponseInvalid => "config get returned invalid content".into(),
            Self::ConfigParseFailed => "config parse failed".into(),
            Self::Generator(error) => error.to_string(),
            Self::ConfigCheckFailed => "config check failed".into(),
            Self::ConfigApplyFailed => "config apply failed".into(),
        }
    }

    fn http_status(&self) -> StatusCode {
        match self {
            Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ServerNotFound => StatusCode::NOT_FOUND,
            Self::ProxyNodeNotFound => StatusCode::NOT_FOUND,
            Self::AgentOffline | Self::AgentUnsupported | Self::AgentDisconnected => StatusCode::CONFLICT,
            Self::AgentQueueFull => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout | Self::OutcomeUnknown => StatusCode::GATEWAY_TIMEOUT,
            Self::ConfigTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::ConfigParseFailed | Self::Generator(_) | Self::ConfigCheckFailed => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Self::ConfigGetFailed | Self::ConfigGetResponseInvalid | Self::ConfigApplyFailed => {
                StatusCode::BAD_GATEWAY
            }
        }
    }

    fn public_message(&self) -> String {
        match self {
            Self::Database(_) => api::INTERNAL.into(),
            Self::ServerNotFound => "服务器不存在".into(),
            Self::ProxyNodeNotFound => "代理节点不存在".into(),
            Self::AgentOffline => "服务器当前离线".into(),
            Self::AgentUnsupported => "当前 agent 不支持 sing-box 配置命令".into(),
            Self::AgentQueueFull => "服务器命令队列已满".into(),
            Self::AgentDisconnected => "服务器连接已断开".into(),
            Self::Timeout => "部署等待 Agent 响应超时".into(),
            Self::OutcomeUnknown => "Agent 连接中断，部署结果未知".into(),
            Self::ConfigTooLarge => "生成的 sing-box 配置超过 32 KiB 限制".into(),
            Self::ConfigGetFailed => "读取 sing-box 配置失败".into(),
            Self::ConfigGetResponseInvalid => "Agent 返回的 sing-box 配置格式不正确".into(),
            Self::ConfigParseFailed => "Agent 返回的 sing-box 配置不是有效 JSON".into(),
            Self::Generator(error) => error.to_string(),
            Self::ConfigCheckFailed => "sing-box 配置校验失败".into(),
            Self::ConfigApplyFailed => "sing-box 配置应用失败".into(),
        }
    }

    fn response(self) -> Response {
        match self {
            Self::Database(error) => api::fail(error),
            error => api::answer(error.http_status(), error.public_message()),
        }
    }
}

fn map_command_error(error: ConfigCommandError, phase: CommandPhase) -> DeployError {
    match error {
        ConfigCommandError::Database(error) => DeployError::Database(error),
        ConfigCommandError::NodeNotFound => DeployError::ServerNotFound,
        ConfigCommandError::AgentOffline => DeployError::AgentOffline,
        ConfigCommandError::AgentUnsupported => DeployError::AgentUnsupported,
        ConfigCommandError::QueueFull => DeployError::AgentQueueFull,
        ConfigCommandError::Disconnected => DeployError::AgentDisconnected,
        ConfigCommandError::Timeout => DeployError::Timeout,
        ConfigCommandError::OutcomeUnknown => DeployError::OutcomeUnknown,
        ConfigCommandError::PayloadTooLarge => DeployError::ConfigTooLarge,
        ConfigCommandError::UnsupportedMethod
        | ConfigCommandError::InvalidParams
        | ConfigCommandError::InvalidResult
        | ConfigCommandError::AgentRejected => match phase {
            CommandPhase::Get => DeployError::ConfigGetFailed,
            CommandPhase::Check => DeployError::ConfigCheckFailed,
            CommandPhase::Apply => DeployError::ConfigApplyFailed,
        },
    }
}

#[derive(Clone, Copy)]
enum CommandPhase {
    Get,
    Check,
    Apply,
}

pub async fn deploy(_: Admin, State(app): State<Shared>, Path(node_id): Path<i64>) -> Response {
    match deploy_server_proxy_config(&app, node_id).await {
        Ok(result) => no_store(
            Json(json!({
                "ok": true,
                "node_id": node_id,
                "changed": result.changed,
                "deployed_nodes": result.deployed_nodes
            }))
            .into_response(),
        ),
        Err(error) => no_store(error.response()),
    }
}

pub async fn remove_proxy_node(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    match remove_proxy_node_and_deploy(&app, id).await {
        Ok(()) => no_store(Json(json!({ "ok": true })).into_response()),
        Err(error) => no_store(error.response()),
    }
}

async fn remove_proxy_node_and_deploy(app: &Shared, id: i64) -> Result<(), DeployError> {
    let initial =
        app.db.proxy_node(id).map_err(DeployError::Database)?.ok_or(DeployError::ProxyNodeNotFound)?;
    let node_id = initial.node_id;
    let lock = app.proxy_deploy_lock(node_id);
    let _operation = lock.lock().await;

    let proxy_node =
        app.db.proxy_node(id).map_err(DeployError::Database)?.ok_or(DeployError::ProxyNodeNotFound)?;
    if proxy_node.node_id != node_id {
        return Err(DeployError::ProxyNodeNotFound);
    }
    match app.db.node(node_id).map_err(DeployError::Database)? {
        Some(_) => {}
        None => return Err(DeployError::ServerNotFound),
    }
    let mut desired_nodes = app.db.proxy_nodes_for_node(node_id).map_err(DeployError::Database)?;
    desired_nodes.retain(|node| node.id != id);
    deploy_nodes_locked(app, node_id, &desired_nodes).await?;

    if !app.db.delete_proxy_node(id).map_err(DeployError::Database)? {
        return Err(DeployError::ProxyNodeNotFound);
    }
    Ok(())
}

async fn deploy_server_proxy_config(app: &Shared, node_id: i64) -> Result<DeployResult, DeployError> {
    let lock = app.proxy_deploy_lock(node_id);
    let _operation = lock.lock().await;

    match app.db.node(node_id).map_err(DeployError::Database)? {
        Some(_) => {}
        None => return Err(DeployError::ServerNotFound),
    }
    let nodes = app.db.proxy_nodes_for_node(node_id).map_err(DeployError::Database)?;
    deploy_nodes_locked(app, node_id, &nodes).await
}

async fn deploy_nodes_locked(
    app: &Shared,
    node_id: i64,
    nodes: &[ProxyNode],
) -> Result<DeployResult, DeployError> {
    app.db.set_proxy_nodes_deploy_status(node_id, "deploying", None).map_err(DeployError::Database)?;

    let result = deploy_configuration(app, node_id, nodes).await;
    match result {
        Ok(changed) => {
            app.db.set_proxy_nodes_deploy_status(node_id, "deployed", None).map_err(DeployError::Database)?;
            Ok(DeployResult { changed, deployed_nodes: nodes.len() })
        }
        Err(error) => {
            let last_error = error.last_error();
            if let Err(database_error) =
                app.db.set_proxy_nodes_deploy_status(node_id, "failed", Some(&last_error))
            {
                tracing::warn!(node_id, "could not persist proxy deployment failure: {database_error:#}");
                return Err(DeployError::Database(database_error));
            }
            Err(error)
        }
    }
}

async fn deploy_configuration(app: &Shared, node_id: i64, nodes: &[ProxyNode]) -> Result<bool, DeployError> {
    let current_response = command::singbox_config_get(app, node_id)
        .await
        .map_err(|error| map_command_error(error, CommandPhase::Get))?;
    let current_content = current_response
        .get("content")
        .and_then(Value::as_str)
        .ok_or(DeployError::ConfigGetResponseInvalid)?;
    let current_config: Value =
        serde_json::from_str(current_content).map_err(|_| DeployError::ConfigParseFailed)?;
    let desired_config =
        generate_singbox_config(node_id, current_config.clone(), nodes).map_err(DeployError::Generator)?;
    if desired_config == current_config {
        return Ok(false);
    }

    let candidate = serde_json::to_string(&desired_config).map_err(|_| DeployError::ConfigApplyFailed)?;
    let check_response = command::singbox_config_check(app, node_id, &candidate)
        .await
        .map_err(|error| map_command_error(error, CommandPhase::Check))?;
    if check_response.get("valid") != Some(&Value::Bool(true)) {
        return Err(DeployError::ConfigCheckFailed);
    }

    let apply_response = command::singbox_config_apply(app, node_id, &candidate)
        .await
        .map_err(|error| map_command_error(error, CommandPhase::Apply))?;
    if apply_response.get("running") != Some(&Value::Bool(true)) {
        return Err(DeployError::ConfigApplyFailed);
    }
    Ok(true)
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_ws::Agent;
    use crate::db::{Db, Node, ProxyNodeConfig};
    use axum::extract::State;
    use serde_json::Value;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio::time::Duration;

    const SESSION: u64 = 73;
    const PRIVATE_SENTINEL: &str = "must-never-be-stored-or-returned";
    const SINGBOX_CONFIG_GET_METHOD: &str = "singbox.config.get";
    const SINGBOX_CONFIG_CHECK_METHOD: &str = "singbox.config.check";
    const SINGBOX_CONFIG_APPLY_METHOD: &str = "singbox.config.apply";

    fn app_node() -> (Shared, i64) {
        let app = Arc::new(App::for_test(Db::open(":memory:").unwrap()));
        let node_id = app
            .db
            .create_node(&Node { name: "proxy-deploy-test".into(), ..Node::default() }, "node-token")
            .unwrap();
        (app, node_id)
    }

    fn proxy_node_config(
        node_id: i64,
        listen_port: u16,
        enabled: bool,
        reality_dest: &str,
    ) -> ProxyNodeConfig {
        ProxyNodeConfig {
            node_id,
            name: format!("proxy-{listen_port}"),
            enabled,
            protocol: "vless_reality".into(),
            address_mode: "ipv4".into(),
            custom_address: None,
            listen_port,
            uuid: "bf000d23-0752-40b4-affe-68f7707a9661".into(),
            reality_private_key: "test-private-key".into(),
            reality_public_key: "test-public-key".into(),
            reality_short_id: "1234abcd".into(),
            reality_server_name: "www.apple.com".into(),
            reality_dest: reality_dest.into(),
        }
    }

    fn create_proxy_node(app: &App, node_id: i64, port: u16, enabled: bool, dest: &str) -> i64 {
        app.db.create_proxy_node(&proxy_node_config(node_id, port, enabled, dest)).unwrap().id
    }

    fn add_agent(app: &App, node_id: i64, session: u64) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(16);
        let mut agent = Agent::new(session, tx);
        agent.capabilities = HashSet::from([
            SINGBOX_CONFIG_GET_METHOD.to_owned(),
            SINGBOX_CONFIG_CHECK_METHOD.to_owned(),
            SINGBOX_CONFIG_APPLY_METHOD.to_owned(),
        ]);
        app.agents.write().unwrap_or_else(|error| error.into_inner()).insert(node_id, agent);
        rx
    }

    fn current_config() -> Value {
        json!({
            "log": {"level": "warn", "timestamp": true},
            "inbounds": [],
            "outbounds": [{"type": "direct", "tag": "direct"}]
        })
    }

    async fn next_request(rx: &mut mpsc::Receiver<String>, expected_method: &str) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("deployment did not send a command")
            .expect("agent command channel closed");
        let request: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(request["method"], expected_method);
        request
    }

    fn reply(app: &App, node_id: i64, session: u64, request: &Value, result: Result<Value, Value>) {
        let id = request["id"].as_str().unwrap();
        assert!(command::complete(app, id, node_id, session, result));
    }

    fn get_result(content: &str) -> Result<Value, Value> {
        Ok(json!({"content": content, "size_bytes": content.len(), "modified_at": 1}))
    }

    fn status(app: &App, proxy_id: i64) -> ProxyNode {
        app.db.proxy_node(proxy_id).unwrap().unwrap()
    }

    async fn complete_successful_deploy(
        app: &Shared,
        node_id: i64,
        rx: &mut mpsc::Receiver<String>,
        current: &str,
    ) -> String {
        let get = next_request(rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(app, node_id, SESSION, &get, get_result(current));
        let check = next_request(rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let checked_content = check["params"]["content"].as_str().unwrap().to_owned();
        reply(app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        let applied_content = apply["params"]["content"].as_str().unwrap().to_owned();
        assert_eq!(checked_content, applied_content, "check and apply must use the same candidate");
        reply(app, node_id, SESSION, &apply, Ok(json!({"running": true})));
        applied_content
    }

    #[tokio::test]
    async fn offline_agent_fails_all_proxy_nodes_without_creating_a_command() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");

        let error = deploy_server_proxy_config(&app, node_id).await.unwrap_err();
        assert!(matches!(error, DeployError::AgentOffline));
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("agent offline"));
    }

    #[tokio::test]
    async fn deploy_aware_delete_applies_configuration_without_the_node_before_removing_db_row() {
        let (app, node_id) = app_node();
        let removed_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let kept_id = create_proxy_node(&app, node_id, 8443, true, "www.apple.com:443");
        let rows = app.db.proxy_nodes_for_node(node_id).unwrap();
        let current = generate_singbox_config(node_id, current_config(), &rows).unwrap().to_string();
        let mut rx = add_agent(&app, node_id, SESSION);
        let remove_app = app.clone();
        let removal =
            tokio::spawn(async move { remove_proxy_node_and_deploy(&remove_app, removed_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let checked_candidate = check["params"]["content"].as_str().unwrap().to_owned();
        assert!(app.db.proxy_node(removed_id).unwrap().is_some());
        reply(&app, node_id, SESSION, &check, Ok(json!({ "valid": true })));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        let candidate = apply["params"]["content"].as_str().unwrap().to_owned();
        assert_eq!(candidate, checked_candidate);
        assert!(app.db.proxy_node(removed_id).unwrap().is_some(), "apply 成功前必须保留数据库记录");
        reply(&app, node_id, SESSION, &apply, Ok(json!({ "running": true })));
        removal.await.unwrap().unwrap();

        let applied: Value = serde_json::from_str(&candidate).unwrap();
        let inbounds = applied["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["tag"], format!("monitor-proxy-node-{kept_id}"));
        assert!(app.db.proxy_node(removed_id).unwrap().is_none());
        assert_eq!(status(&app, kept_id).deploy_status, "deployed");
    }

    #[tokio::test]
    async fn delete_keeps_database_row_when_config_check_fails() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let rows = app.db.proxy_nodes_for_node(node_id).unwrap();
        let current = generate_singbox_config(node_id, current_config(), &rows).unwrap().to_string();
        let mut rx = add_agent(&app, node_id, SESSION);
        let remove_app = app.clone();
        let removal = tokio::spawn(async move { remove_proxy_node_and_deploy(&remove_app, proxy_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));
        assert!(matches!(removal.await.unwrap(), Err(DeployError::ConfigCheckFailed)));
        assert!(rx.try_recv().is_err());
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("config check failed"));
    }

    #[tokio::test]
    async fn delete_keeps_database_row_when_config_apply_fails() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let rows = app.db.proxy_nodes_for_node(node_id).unwrap();
        let current = generate_singbox_config(node_id, current_config(), &rows).unwrap().to_string();
        let mut rx = add_agent(&app, node_id, SESSION);
        let remove_app = app.clone();
        let removal = tokio::spawn(async move { remove_proxy_node_and_deploy(&remove_app, proxy_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        reply(&app, node_id, SESSION, &apply, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));
        assert!(matches!(removal.await.unwrap(), Err(DeployError::ConfigApplyFailed)));
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("config apply failed"));
    }

    #[tokio::test]
    async fn delete_keeps_database_row_when_agent_is_offline() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");

        assert!(matches!(remove_proxy_node_and_deploy(&app, proxy_id).await, Err(DeployError::AgentOffline)));
        assert!(app.db.proxy_node(proxy_id).unwrap().is_some());
        assert_eq!(status(&app, proxy_id).deploy_status, "failed");
    }

    #[tokio::test]
    async fn config_get_failure_is_summarized_without_agent_error_text() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));
        let error = deployment.await.unwrap().unwrap_err();
        assert!(matches!(error, DeployError::ConfigGetFailed));
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        let summary = saved.last_error.unwrap();
        assert_eq!(summary, "config get failed");
        assert!(!summary.contains(PRIVATE_SENTINEL));
    }

    #[tokio::test]
    async fn invalid_json_from_config_get_fails_before_check_or_apply() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result("{not-json"));
        let error = deployment.await.unwrap().unwrap_err();
        assert!(matches!(error, DeployError::ConfigParseFailed));
        assert!(rx.try_recv().is_err());
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("config parse failed"));
    }

    #[tokio::test]
    async fn generator_failure_is_saved_without_any_secret_context() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "2001:db8::1:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        let initial = current_config().to_string();
        reply(&app, node_id, SESSION, &get, get_result(&initial));
        let error = deployment.await.unwrap().unwrap_err();
        assert!(matches!(error, DeployError::Generator(ProxyConfigError::InvalidRealityDest)));
        assert!(rx.try_recv().is_err());
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        let summary = saved.last_error.unwrap();
        assert!(summary.contains("Reality Dest"));
        assert!(!summary.contains("test-private-key"));
    }

    #[tokio::test]
    async fn config_check_failure_stops_before_apply_and_saves_safe_summary() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        let initial = current_config().to_string();
        reply(&app, node_id, SESSION, &get, get_result(&initial));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        assert!(check["params"]["content"].is_string());
        reply(&app, node_id, SESSION, &check, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));
        let error = deployment.await.unwrap().unwrap_err();
        assert!(matches!(error, DeployError::ConfigCheckFailed));
        assert!(rx.try_recv().is_err());
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("config check failed"));
        assert!(!saved.last_error.unwrap().contains(PRIVATE_SENTINEL));
    }

    #[tokio::test]
    async fn config_apply_failure_is_summarized_and_marks_nodes_failed() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        let initial = current_config().to_string();
        reply(&app, node_id, SESSION, &get, get_result(&initial));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        reply(&app, node_id, SESSION, &apply, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));
        let error = deployment.await.unwrap().unwrap_err();
        assert!(matches!(error, DeployError::ConfigApplyFailed));
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.last_error.as_deref(), Some("config apply failed"));
        assert!(!saved.last_error.unwrap().contains(PRIVATE_SENTINEL));
    }

    #[tokio::test]
    async fn successful_apply_marks_nodes_deployed_and_checks_the_same_candidate() {
        let (app, node_id) = app_node();
        let first = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let second = create_proxy_node(&app, node_id, 8443, false, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });
        let initial = current_config().to_string();
        let candidate = complete_successful_deploy(&app, node_id, &mut rx, &initial).await;

        let result = deployment.await.unwrap().unwrap();
        assert!(result.changed);
        assert_eq!(result.deployed_nodes, 2, "count all database rows, including disabled nodes");
        assert_eq!(status(&app, first).deploy_status, "deployed");
        assert_eq!(status(&app, first).last_error, None);
        assert_eq!(status(&app, second).deploy_status, "deployed");
        let config: Value = serde_json::from_str(&candidate).unwrap();
        assert_eq!(config["inbounds"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn semantically_unchanged_config_skips_check_and_apply() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let nodes = app.db.proxy_nodes_for_node(node_id).unwrap();
        let current = generate_singbox_config(node_id, current_config(), &nodes).unwrap().to_string();
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let result = deployment.await.unwrap().unwrap();
        assert!(!result.changed);
        assert_eq!(result.deployed_nodes, 1);
        assert!(rx.try_recv().is_err());
        let saved = status(&app, proxy_id);
        assert_eq!(saved.deploy_status, "deployed");
        assert_eq!(saved.last_error, None);
    }

    #[tokio::test]
    async fn deploying_a_disabled_node_removes_its_old_managed_inbound() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, false, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let deployment = tokio::spawn(async move { deploy_server_proxy_config(&deploy_app, node_id).await });
        let initial = json!({
            "log": {"level": "warn"},
            "inbounds": [
                {"type": "socks", "tag": "user-inbound"},
                {"type": "vless", "tag": "monitor-proxy-node-1", "old": true}
            ],
            "outbounds": [{"type": "direct", "tag": "direct"}]
        })
        .to_string();
        let candidate = complete_successful_deploy(&app, node_id, &mut rx, &initial).await;
        deployment.await.unwrap().unwrap();

        let applied: Value = serde_json::from_str(&candidate).unwrap();
        let inbounds = applied["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["tag"], "user-inbound");
        assert_eq!(status(&app, proxy_id).deploy_status, "deployed");
    }

    #[tokio::test]
    async fn same_server_deployments_are_serialized_across_the_full_transaction() {
        let (app, node_id) = app_node();
        create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);

        let first_app = app.clone();
        let first = tokio::spawn(async move { deploy_server_proxy_config(&first_app, node_id).await });
        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        let second_app = app.clone();
        let second = tokio::spawn(async move { deploy_server_proxy_config(&second_app, node_id).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rx.try_recv().is_err(), "a second config.get must wait for the first deployment");

        let initial = current_config().to_string();
        reply(&app, node_id, SESSION, &get, get_result(&initial));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        let candidate = apply["params"]["content"].as_str().unwrap().to_owned();
        reply(&app, node_id, SESSION, &apply, Ok(json!({"running": true})));
        assert!(first.await.unwrap().unwrap().changed);

        let next_get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &next_get, get_result(&candidate));
        assert!(!second.await.unwrap().unwrap().changed);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn different_servers_can_deploy_without_waiting_on_each_other() {
        let (app, first_node_id) = app_node();
        let second_node_id = app
            .db
            .create_node(&Node { name: "second-proxy-server".into(), ..Node::default() }, "second-token")
            .unwrap();
        create_proxy_node(&app, first_node_id, 443, true, "www.apple.com:443");
        create_proxy_node(&app, second_node_id, 443, true, "www.apple.com:443");
        let mut first_rx = add_agent(&app, first_node_id, SESSION);
        let mut second_rx = add_agent(&app, second_node_id, SESSION + 1);

        let first_app = app.clone();
        let first = tokio::spawn(async move { deploy_server_proxy_config(&first_app, first_node_id).await });
        let second_app = app.clone();
        let second =
            tokio::spawn(async move { deploy_server_proxy_config(&second_app, second_node_id).await });

        let first_get = next_request(&mut first_rx, SINGBOX_CONFIG_GET_METHOD).await;
        let second_get = next_request(&mut second_rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, first_node_id, SESSION, &first_get, get_result(&current_config().to_string()));
        reply(&app, second_node_id, SESSION + 1, &second_get, get_result(&current_config().to_string()));

        let first_check = next_request(&mut first_rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let second_check = next_request(&mut second_rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, first_node_id, SESSION, &first_check, Ok(json!({"valid": true})));
        reply(&app, second_node_id, SESSION + 1, &second_check, Ok(json!({"valid": true})));

        let first_apply = next_request(&mut first_rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        let second_apply = next_request(&mut second_rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        reply(&app, first_node_id, SESSION, &first_apply, Ok(json!({"running": true})));
        reply(&app, second_node_id, SESSION + 1, &second_apply, Ok(json!({"running": true})));
        assert!(first.await.unwrap().unwrap().changed);
        assert!(second.await.unwrap().unwrap().changed);
    }

    #[tokio::test]
    async fn admin_deploy_endpoint_returns_a_no_store_summary() {
        let (app, node_id) = app_node();
        create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let mut rx = add_agent(&app, node_id, SESSION);
        let deploy_app = app.clone();
        let response_task =
            tokio::spawn(async move { deploy(Admin, State(deploy_app), Path(node_id)).await });
        let initial = current_config().to_string();
        complete_successful_deploy(&app, node_id, &mut rx, &initial).await;
        let response = response_task.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value, json!({"ok": true, "node_id": node_id, "changed": true, "deployed_nodes": 1}));
    }
}
