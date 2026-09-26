//! 手动触发的服务器级 ProxyNode 配置部署。

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{self, Admin};
use crate::command::{self, ConfigCommandError};
use crate::db::{Node, ProxyNode, ProxyNodeConfig};
use crate::proxy_config::{generate_singbox_config_excluding_users, ProxyConfigError};
use crate::proxy_import::{self, ProxyImportCandidate, ProxyImportRequest};
#[cfg(test)]
use crate::App;
use crate::Shared;

#[derive(Debug)]
struct DeployResult {
    changed: bool,
    deployed_nodes: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ServerDeployFailure {
    pub server_id: i64,
    pub server_name: String,
    pub error: String,
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
    ApplyOutcomeUnknown,
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
            Self::ApplyOutcomeUnknown => "deployment apply outcome unknown".into(),
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
            Self::Timeout | Self::OutcomeUnknown | Self::ApplyOutcomeUnknown => StatusCode::GATEWAY_TIMEOUT,
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
            Self::ApplyOutcomeUnknown => "Agent 响应中断，配置应用结果未知；请重新部署确认".into(),
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

/// 对用户授权、状态或 UUID 变更涉及的服务器逐台复用现有配置部署服务。
pub(crate) async fn deploy_proxy_user_servers(app: &Shared, server_ids: &[i64]) -> Vec<ServerDeployFailure> {
    let ids = server_ids.iter().copied().filter(|id| *id > 0).collect::<std::collections::BTreeSet<_>>();
    let mut failures = Vec::new();
    for server_id in ids {
        if let Err(error) = deploy_server_proxy_config(app, server_id).await {
            let server_name = match app.db.node(server_id) {
                Ok(Some(node)) => node.name,
                Ok(None) => format!("服务器 {server_id}"),
                Err(database_error) => {
                    tracing::warn!(
                        server_id,
                        "could not read server name for deployment result: {database_error:#}"
                    );
                    format!("服务器 {server_id}")
                }
            };
            failures.push(ServerDeployFailure { server_id, server_name, error: error.public_message() });
        }
    }
    failures
}

fn map_command_error(error: ConfigCommandError, phase: CommandPhase) -> DeployError {
    match error {
        ConfigCommandError::Database(error) => DeployError::Database(error),
        ConfigCommandError::NodeNotFound => DeployError::ServerNotFound,
        ConfigCommandError::AgentOffline => DeployError::AgentOffline,
        ConfigCommandError::AgentUnsupported => DeployError::AgentUnsupported,
        ConfigCommandError::QueueFull => DeployError::AgentQueueFull,
        ConfigCommandError::Disconnected => match phase {
            CommandPhase::Apply => DeployError::ApplyOutcomeUnknown,
            _ => DeployError::AgentDisconnected,
        },
        ConfigCommandError::Timeout => match phase {
            CommandPhase::Apply => DeployError::ApplyOutcomeUnknown,
            _ => DeployError::Timeout,
        },
        ConfigCommandError::OutcomeUnknown => match phase {
            CommandPhase::Apply => DeployError::ApplyOutcomeUnknown,
            _ => DeployError::OutcomeUnknown,
        },
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

pub async fn scan_proxy_imports(_: Admin, State(app): State<Shared>, Path(node_id): Path<i64>) -> Response {
    let lock = app.proxy_deploy_lock(node_id);
    let _operation = lock.lock().await;
    let Some(server) = (match app.db.node(node_id) {
        Ok(server) => server,
        Err(error) => return no_store(api::fail(error)),
    }) else {
        return no_store(api::answer(StatusCode::NOT_FOUND, "服务器不存在"));
    };
    let (config, content) = match read_singbox_config(&app, node_id).await {
        Ok(config) => config,
        Err(error) => return no_store(error.response()),
    };
    let mut candidates = match proxy_import::scan_config(&config) {
        Ok(candidates) => candidates,
        Err(error) => return no_store(api::answer(StatusCode::UNPROCESSABLE_ENTITY, error)),
    };
    for candidate in &mut candidates {
        set_suggested_address(candidate, &server);
        if candidate.importable {
            if let Some(tag) = candidate.source_tag.as_deref() {
                match app.db.proxy_node_by_source_tag(node_id, tag) {
                    Ok(Some(_)) => {
                        candidate.importable = false;
                        candidate.reason = Some("该 inbound 有待确认的接管记录，请先重新部署".into());
                    }
                    Ok(None) => {}
                    Err(error) => return no_store(api::fail(error)),
                }
            }
        }
    }
    no_store(
        Json(json!({
            "config_fingerprint": crate::auth::sha256(&content),
            "inbounds": candidates
        }))
        .into_response(),
    )
}

pub async fn import_proxy_inbound(
    _: Admin,
    State(app): State<Shared>,
    Path(node_id): Path<i64>,
    body: Result<Json<ProxyImportRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "导入请求格式不正确"));
    };
    if request.name.trim().is_empty() || request.config_fingerprint.len() != 64 {
        return no_store(api::answer(StatusCode::BAD_REQUEST, "请填写节点名称并重新扫描配置"));
    }

    let lock = app.proxy_deploy_lock(node_id);
    let _operation = lock.lock().await;
    let Some(server) = (match app.db.node(node_id) {
        Ok(server) => server,
        Err(error) => return no_store(api::fail(error)),
    }) else {
        return no_store(api::answer(StatusCode::NOT_FOUND, "服务器不存在"));
    };
    let (current_config, current_content) = match read_singbox_config(&app, node_id).await {
        Ok(config) => config,
        Err(error) => return no_store(error.response()),
    };
    if crate::auth::sha256(&current_content) != request.config_fingerprint {
        return no_store(api::answer(StatusCode::CONFLICT, "配置已变化，请重新扫描"));
    }
    if request.source_tag.starts_with(crate::proxy_config::MANAGED_INBOUND_TAG_PREFIX) {
        return no_store(api::answer(StatusCode::UNPROCESSABLE_ENTITY, "Monitor 已管理该 inbound"));
    }
    let imported = match proxy_import::find_importable(&current_config, &request.source_tag) {
        Ok(imported) => imported,
        Err(reason) => return no_store(api::answer(StatusCode::UNPROCESSABLE_ENTITY, reason)),
    };
    match app.db.proxy_node_by_source_tag(node_id, &request.source_tag) {
        Ok(Some(_)) => {
            return no_store(api::answer(StatusCode::CONFLICT, "该 inbound 已有待确认的接管记录"));
        }
        Ok(None) => {}
        Err(error) => return no_store(api::fail(error)),
    }
    let (address_mode, custom_address) = match selected_connection_address(&server, &request) {
        Ok(address) => address,
        Err(message) => return no_store(api::answer(StatusCode::UNPROCESSABLE_ENTITY, message)),
    };

    let mut desired_nodes = match app.db.proxy_nodes_for_node(node_id) {
        Ok(nodes) => nodes,
        Err(error) => return no_store(api::fail(error)),
    };
    if desired_nodes.iter().any(|node| node.listen_port == imported.listen_port) {
        return no_store(api::answer(StatusCode::CONFLICT, "该监听端口已被 ProxyNode 占用"));
    }

    let config = ProxyNodeConfig {
        node_id,
        name: request.name.trim().to_owned(),
        enabled: true,
        protocol: "vless_reality".into(),
        address_mode,
        custom_address,
        listen_port: imported.listen_port,
        uuid: imported.uuid,
        reality_private_key: imported.reality_private_key,
        reality_public_key: imported.reality_public_key,
        reality_short_id: imported.reality_short_id,
        reality_server_name: imported.reality_server_name,
        reality_dest: imported.reality_dest,
    };
    let created =
        match app.db.create_imported_proxy_node(&config, &imported.listen_address, &imported.source_tag) {
            Ok(created) => created,
            Err(error) => return no_store(api::fail(error)),
        };
    desired_nodes.push(created.clone());

    match deploy_configuration_from_current(&app, node_id, &desired_nodes, &current_config, &[]).await {
        Ok(_) => match complete_import(&app, node_id, created.id) {
            Ok(node) => no_store((StatusCode::CREATED, Json(json!({ "node": node }))).into_response()),
            Err(error) => {
                mark_import_failed(&app, created.id, "配置已应用，但数据库状态未能更新");
                no_store(error.response())
            }
        },
        Err(error) if matches!(&error, DeployError::ApplyOutcomeUnknown) => {
            match read_singbox_config(&app, node_id).await {
                Ok((observed, _)) => {
                    match inspect_import_outcome(&observed, &imported.source_tag, created.id) {
                        ImportOutcome::Applied => match complete_import(&app, node_id, created.id) {
                            Ok(node) => {
                                no_store((StatusCode::CREATED, Json(json!({ "node": node }))).into_response())
                            }
                            Err(database_error) => {
                                mark_import_failed(&app, created.id, "配置已应用，但数据库状态未能更新");
                                no_store(database_error.response())
                            }
                        },
                        ImportOutcome::NotApplied => {
                            if let Err(database_error) = app.db.delete_proxy_node(created.id) {
                                return no_store(DeployError::Database(database_error).response());
                            }
                            no_store(error.response())
                        }
                        ImportOutcome::Unclear => {
                            mark_import_failed(&app, created.id, &error.last_error());
                            no_store(error.response())
                        }
                    }
                }
                Err(_) => {
                    mark_import_failed(&app, created.id, &error.last_error());
                    no_store(error.response())
                }
            }
        }
        Err(error) => {
            if let Err(database_error) = app.db.delete_proxy_node(created.id) {
                return no_store(DeployError::Database(database_error).response());
            }
            no_store(error.response())
        }
    }
}

fn set_suggested_address(candidate: &mut ProxyImportCandidate, server: &Node) {
    let addresses =
        api::addresses(&server.ip, (&server.ipv4, &server.ipv6), (&server.ipv4_pin, &server.ipv6_pin));
    let ipv4 = addresses
        .iter()
        .find(|(address, _)| address.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_ipv4()))
        .map(|(address, _)| (*address).to_owned());
    let ipv6 = addresses
        .iter()
        .find(|(address, _)| address.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_ipv6()))
        .map(|(address, _)| (*address).to_owned());
    if let Some(address) = ipv4 {
        candidate.suggested_address_mode = Some("ipv4".into());
        candidate.suggested_address = Some(address);
    } else if let Some(address) = ipv6 {
        candidate.suggested_address_mode = Some("ipv6".into());
        candidate.suggested_address = Some(address);
    }
}

fn selected_connection_address(
    server: &Node,
    request: &ProxyImportRequest,
) -> Result<(String, Option<String>), &'static str> {
    let custom_address = request.custom_address.as_deref().map(str::trim).filter(|value| !value.is_empty());
    match request.address_mode.as_str() {
        "ipv4" => server_address(server, false)
            .map(|_| ("ipv4".into(), None))
            .ok_or("所选服务器没有可用 IPv4，请选择 IPv6 或自定义地址"),
        "ipv6" => server_address(server, true)
            .map(|_| ("ipv6".into(), None))
            .ok_or("所选服务器没有可用 IPv6，请选择 IPv4 或自定义地址"),
        "custom" if custom_address.is_some() => Ok(("custom".into(), custom_address.map(str::to_owned))),
        "custom" => Err("请填写自定义连接地址"),
        _ => Err("连接地址模式无效"),
    }
}

fn server_address(server: &Node, ipv6: bool) -> Option<&str> {
    api::addresses(&server.ip, (&server.ipv4, &server.ipv6), (&server.ipv4_pin, &server.ipv6_pin))
        .into_iter()
        .find(|(address, _)| address.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_ipv6() == ipv6))
        .map(|(address, _)| address)
}

enum ImportOutcome {
    Applied,
    NotApplied,
    Unclear,
}

fn inspect_import_outcome(config: &Value, source_tag: &str, proxy_node_id: i64) -> ImportOutcome {
    let Some(inbounds) = config.get("inbounds").and_then(Value::as_array) else {
        return ImportOutcome::Unclear;
    };
    let source_exists =
        inbounds.iter().any(|inbound| inbound.get("tag").and_then(Value::as_str) == Some(source_tag));
    let managed_tag = format!("{}{}", crate::proxy_config::MANAGED_INBOUND_TAG_PREFIX, proxy_node_id);
    let managed_exists = inbounds
        .iter()
        .any(|inbound| inbound.get("tag").and_then(Value::as_str) == Some(managed_tag.as_str()));
    match (source_exists, managed_exists) {
        (false, true) => ImportOutcome::Applied,
        (true, false) => ImportOutcome::NotApplied,
        _ => ImportOutcome::Unclear,
    }
}

fn complete_import(app: &Shared, node_id: i64, proxy_node_id: i64) -> Result<ProxyNode, DeployError> {
    app.db.set_proxy_nodes_deploy_status(node_id, "deployed", None).map_err(DeployError::Database)?;
    app.db.clear_proxy_node_source_tags(node_id).map_err(DeployError::Database)?;
    app.db.proxy_node(proxy_node_id).map_err(DeployError::Database)?.ok_or(DeployError::ProxyNodeNotFound)
}

fn mark_import_failed(app: &Shared, proxy_node_id: i64, last_error: &str) {
    if let Err(error) = app.db.set_proxy_node_deploy_status(proxy_node_id, "failed", Some(last_error)) {
        tracing::warn!(proxy_node_id, "could not persist pending proxy import state: {error:#}");
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
    let removed_source_tag = proxy_node.source_inbound_tag.clone().into_iter().collect::<Vec<_>>();
    deploy_nodes_locked(app, node_id, &desired_nodes, &removed_source_tag).await?;

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
    deploy_nodes_locked(app, node_id, &nodes, &[]).await
}

async fn deploy_nodes_locked(
    app: &Shared,
    node_id: i64,
    nodes: &[ProxyNode],
    additionally_excluded_tags: &[String],
) -> Result<DeployResult, DeployError> {
    app.db.set_proxy_nodes_deploy_status(node_id, "deploying", None).map_err(DeployError::Database)?;

    let result = deploy_configuration(app, node_id, nodes, additionally_excluded_tags).await;
    match result {
        Ok(changed) => {
            app.db.set_proxy_nodes_deploy_status(node_id, "deployed", None).map_err(DeployError::Database)?;
            app.db.clear_proxy_node_source_tags(node_id).map_err(DeployError::Database)?;
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

async fn deploy_configuration(
    app: &Shared,
    node_id: i64,
    nodes: &[ProxyNode],
    additionally_excluded_tags: &[String],
) -> Result<bool, DeployError> {
    let (current_config, _) = read_singbox_config(app, node_id).await?;
    deploy_configuration_from_current(app, node_id, nodes, &current_config, additionally_excluded_tags).await
}

async fn deploy_configuration_from_current(
    app: &Shared,
    node_id: i64,
    nodes: &[ProxyNode],
    current_config: &Value,
    additionally_excluded_tags: &[String],
) -> Result<bool, DeployError> {
    let users = app.db.proxy_users_for_node(node_id).map_err(DeployError::Database)?;
    let desired_config = generate_singbox_config_excluding_users(
        node_id,
        current_config.clone(),
        nodes,
        &users,
        additionally_excluded_tags,
    )
    .map_err(DeployError::Generator)?;
    if desired_config == *current_config {
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

async fn read_singbox_config(app: &Shared, node_id: i64) -> Result<(Value, String), DeployError> {
    let current_response = command::singbox_config_get(app, node_id)
        .await
        .map_err(|error| map_command_error(error, CommandPhase::Get))?;
    let current_content = current_response
        .get("content")
        .and_then(Value::as_str)
        .ok_or(DeployError::ConfigGetResponseInvalid)?;
    let current_config: Value =
        serde_json::from_str(current_content).map_err(|_| DeployError::ConfigParseFailed)?;
    Ok((current_config, current_content.to_owned()))
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_ws::Agent;
    use crate::db::{Db, Node, NodePatch, ProxyNodeConfig};
    use crate::proxy_config::generate_singbox_config;
    use axum::body::to_bytes;
    use axum::extract::State;
    use axum::http::StatusCode;
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

    fn create_proxy_user(app: &App, proxy_node_id: i64) -> i64 {
        app.db
            .create_proxy_user_with_nodes(
                "Alice",
                "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
                true,
                "",
                &[proxy_node_id],
            )
            .unwrap()
            .0
            .id
    }

    fn current_config_with_user(app: &App, node_id: i64) -> String {
        let nodes = app.db.proxy_nodes_for_node(node_id).unwrap();
        let users = app.db.proxy_users_for_node(node_id).unwrap();
        crate::proxy_config::generate_singbox_config_excluding_users(
            node_id,
            current_config(),
            &nodes,
            &users,
            &[],
        )
        .unwrap()
        .to_string()
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

    fn import_config() -> Value {
        json!({
            "log": {"level": "warn", "timestamp": true},
            "inbounds": [{
                "type": "vless",
                "tag": "legacy-reality",
                "listen": "0.0.0.0",
                "listen_port": 33333,
                "tls": {
                    "enabled": true,
                    "server_name": "www.amd.com",
                    "reality": {
                        "enabled": true,
                        "handshake": {"server": "www.amd.com", "server_port": 443},
                        "private_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "short_id": ["3efe85475d460e65"]
                    }
                },
                "users": [{
                    "flow": "xtls-rprx-vision",
                    "name": "legacy-user",
                    "uuid": "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e"
                }]
            }],
            "outbounds": [{"type": "direct", "tag": "direct"}]
        })
    }

    fn import_request(fingerprint: String) -> ProxyImportRequest {
        ProxyImportRequest {
            config_fingerprint: fingerprint,
            source_tag: "legacy-reality".into(),
            name: "Imported Reality".into(),
            address_mode: "ipv4".into(),
            custom_address: None,
        }
    }

    async fn response_json(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn next_import_request(
        rx: &mut mpsc::Receiver<String>,
        importing: &mut tokio::task::JoinHandle<Response>,
        expected_method: &str,
    ) -> Value {
        tokio::select! {
            request = next_request(rx, expected_method) => request,
            response = importing => {
                let response = response.unwrap();
                let status = response.status();
                let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                panic!("import returned before {expected_method}: {status} {}", String::from_utf8_lossy(&body));
            }
        }
    }

    fn app_with_import_address() -> (Shared, i64) {
        let app = Arc::new(App::for_test(Db::open(":memory:").unwrap()));
        let node_id = app
            .db
            .create_node(
                &Node {
                    name: "proxy-import-test".into(),
                    ipv4_pin: "203.0.113.15".into(),
                    ..Node::default()
                },
                "node-token",
            )
            .unwrap();
        app.db
            .update_node(
                node_id,
                &NodePatch { ipv4_pin: Some("203.0.113.15".into()), ..NodePatch::default() },
            )
            .unwrap();
        (app, node_id)
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
    async fn scan_endpoint_returns_a_fingerprinted_preview_without_private_key() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let scan_app = app.clone();
        let scan =
            tokio::spawn(async move { scan_proxy_imports(Admin, State(scan_app), Path(node_id)).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let response = scan.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = response_json(response).await;
        assert_eq!(body["config_fingerprint"], crate::auth::sha256(&content));
        assert_eq!(body["inbounds"][0]["source_tag"], "legacy-reality");
        assert_eq!(body["inbounds"][0]["suggested_address_mode"], "ipv4");
        assert_eq!(body["inbounds"][0]["suggested_address"], "203.0.113.15");
        let serialized = body.to_string();
        assert!(!serialized.contains("private_key"));
        assert!(!serialized.contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
        assert!(rx.try_recv().is_err(), "scan must not check or apply a config");
    }

    #[tokio::test]
    async fn import_takes_over_the_source_and_persists_only_after_successful_apply() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let request = import_request(crate::auth::sha256(&content));
        let import_app = app.clone();
        let mut importing = tokio::spawn(async move {
            import_proxy_inbound(Admin, State(import_app), Path(node_id), Ok(Json(request))).await
        });

        let get = next_import_request(&mut rx, &mut importing, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let checked = check["params"]["content"].as_str().unwrap().to_owned();
        let candidate: Value = serde_json::from_str(&checked).unwrap();
        let inbounds = candidate["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0]["tag"], "monitor-proxy-node-1");
        assert_eq!(inbounds[0]["listen"], "0.0.0.0");
        assert_eq!(inbounds[0]["listen_port"], 33333);
        assert!(inbounds.iter().all(|inbound| inbound["tag"] != "legacy-reality"));
        assert_eq!(app.db.proxy_nodes_for_node(node_id).unwrap().len(), 1);
        let staged = app.db.proxy_nodes_for_node(node_id).unwrap().remove(0);
        assert_eq!(staged.deploy_status, "deploying");
        assert_eq!(staged.source_inbound_tag.as_deref(), Some("legacy-reality"));

        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        assert_eq!(apply["params"]["content"], checked);
        reply(&app, node_id, SESSION, &apply, Ok(json!({"running": true})));

        let response = importing.await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response_json(response).await;
        assert_eq!(body["node"]["deploy_status"], "deployed");
        assert_eq!(body["node"]["listen_address"], "0.0.0.0");
        assert!(body.to_string().find("reality_private_key").is_none());
        assert!(body.to_string().find("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_none());
        let saved = app.db.proxy_nodes_for_node(node_id).unwrap().remove(0);
        assert_eq!(saved.deploy_status, "deployed");
        assert_eq!(saved.source_inbound_tag, None);
        assert_eq!(saved.listen_address, "0.0.0.0");

        let redeploy_app = app.clone();
        let redeployment =
            tokio::spawn(async move { deploy_server_proxy_config(&redeploy_app, node_id).await });
        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&checked));
        let result = redeployment.await.unwrap().unwrap();
        assert!(!result.changed, "the first ordinary deploy after takeover must be idempotent");
        assert_eq!(result.deployed_nodes, 1);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn scan_suggests_ipv6_when_no_ipv4_is_available() {
        let (app, node_id) = app_with_import_address();
        app.db
            .update_node(
                node_id,
                &NodePatch {
                    ipv4_pin: Some(String::new()),
                    ipv6_pin: Some("2001:db8::15".into()),
                    ..NodePatch::default()
                },
            )
            .unwrap();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let scan_app = app.clone();
        let scan =
            tokio::spawn(async move { scan_proxy_imports(Admin, State(scan_app), Path(node_id)).await });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let body = response_json(scan.await.unwrap()).await;
        assert_eq!(body["inbounds"][0]["suggested_address_mode"], "ipv6");
        assert_eq!(body["inbounds"][0]["suggested_address"], "2001:db8::15");
    }

    #[test]
    fn import_address_selection_rejects_missing_server_families() {
        let mut request = import_request("a".repeat(64));
        assert_eq!(
            selected_connection_address(&Node::default(), &request).unwrap_err(),
            "所选服务器没有可用 IPv4，请选择 IPv6 或自定义地址"
        );
        request.address_mode = "ipv6".into();
        assert_eq!(
            selected_connection_address(&Node::default(), &request).unwrap_err(),
            "所选服务器没有可用 IPv6，请选择 IPv4 或自定义地址"
        );
    }

    #[tokio::test]
    async fn import_check_failure_does_not_leave_a_proxy_node_or_apply_config() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let request = import_request(crate::auth::sha256(&content));
        let import_app = app.clone();
        let importing = tokio::spawn(async move {
            import_proxy_inbound(Admin, State(import_app), Path(node_id), Ok(Json(request))).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));

        let response = importing.await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(app.db.proxy_nodes_for_node(node_id).unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "failed config.check must stop before apply");
    }

    #[tokio::test]
    async fn import_apply_failure_does_not_leave_a_proxy_node() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let request = import_request(crate::auth::sha256(&content));
        let import_app = app.clone();
        let importing = tokio::spawn(async move {
            import_proxy_inbound(Admin, State(import_app), Path(node_id), Ok(Json(request))).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        reply(&app, node_id, SESSION, &apply, Err(json!({"code": -32000, "message": PRIVATE_SENTINEL})));

        let response = importing.await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(app.db.proxy_nodes_for_node(node_id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_rejects_changed_preview_before_creating_a_proxy_node() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let changed_content = import_config().to_string();
        let stale_fingerprint = crate::auth::sha256("previous-config");
        let request = import_request(stale_fingerprint);
        let import_app = app.clone();
        let importing = tokio::spawn(async move {
            import_proxy_inbound(Admin, State(import_app), Path(node_id), Ok(Json(request))).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&changed_content));
        let response = importing.await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(app.db.proxy_nodes_for_node(node_id).unwrap().is_empty());
        assert!(rx.try_recv().is_err(), "a stale scan must not check or apply config");
    }

    #[tokio::test]
    async fn uncertain_import_apply_keeps_a_failed_source_tag_record_for_retry() {
        let (app, node_id) = app_with_import_address();
        let mut rx = add_agent(&app, node_id, SESSION);
        let content = import_config().to_string();
        let request = import_request(crate::auth::sha256(&content));
        let import_app = app.clone();
        let importing = tokio::spawn(async move {
            import_proxy_inbound(Admin, State(import_app), Path(node_id), Ok(Json(request))).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&content));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({"valid": true})));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        command::disconnect(&app, node_id, SESSION);
        let outcome_check = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &outcome_check, get_result(&current_config().to_string()));

        let response = importing.await.unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let saved = app.db.proxy_nodes_for_node(node_id).unwrap().remove(0);
        assert_eq!(saved.deploy_status, "failed");
        assert_eq!(saved.source_inbound_tag.as_deref(), Some("legacy-reality"));
        assert_eq!(saved.last_error.as_deref(), Some("deployment apply outcome unknown"));
        assert!(rx.try_recv().is_err());
        assert_eq!(apply["method"], SINGBOX_CONFIG_APPLY_METHOD);
    }

    #[tokio::test]
    async fn import_with_an_offline_agent_does_not_create_a_proxy_node() {
        let (app, node_id) = app_with_import_address();
        let content = import_config().to_string();
        let request = import_request(crate::auth::sha256(&content));
        let response =
            import_proxy_inbound(Admin, State(app.clone()), Path(node_id), Ok(Json(request))).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(app.db.proxy_nodes_for_node(node_id).unwrap().is_empty());
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
    async fn deploy_aware_user_delete_removes_credentials_before_deleting_database_user() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let user_id = create_proxy_user(&app, proxy_id);
        let current = current_config_with_user(&app, node_id);
        let mut rx = add_agent(&app, node_id, SESSION);
        let delete_app = app.clone();
        let deletion = tokio::spawn(async move {
            crate::proxy_user::delete_proxy_user(Admin, State(delete_app), Path(user_id)).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let checked = check["params"]["content"].as_str().unwrap().to_owned();
        let candidate: Value = serde_json::from_str(&checked).unwrap();
        assert_eq!(candidate["inbounds"][0]["users"].as_array().unwrap().len(), 1);
        assert!(app.db.proxy_user(user_id).unwrap().is_some(), "数据库记录必须等 apply 成功后才删除");
        reply(&app, node_id, SESSION, &check, Ok(json!({ "valid": true })));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        assert_eq!(apply["params"]["content"], checked);
        assert!(app.db.proxy_user(user_id).unwrap().is_some());
        reply(&app, node_id, SESSION, &apply, Ok(json!({ "running": true })));

        let response = deletion.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["deleted"], true);
        assert!(app.db.proxy_user(user_id).unwrap().is_none());
    }

    #[tokio::test]
    async fn user_delete_keeps_disabled_user_when_config_check_fails() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let user_id = create_proxy_user(&app, proxy_id);
        let current = current_config_with_user(&app, node_id);
        let mut rx = add_agent(&app, node_id, SESSION);
        let delete_app = app.clone();
        let deletion = tokio::spawn(async move {
            crate::proxy_user::delete_proxy_user(Admin, State(delete_app), Path(user_id)).await
        });
        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Err(json!({ "code": -32000, "message": PRIVATE_SENTINEL })));

        let response = deletion.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["deleted"], false);
        assert_eq!(body["user"]["enabled"], false);
        assert_eq!(body["failed_servers"][0]["server_id"], node_id);
        let saved = app.db.proxy_user(user_id).unwrap().unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.proxy_node_ids, [proxy_id]);
        assert!(rx.try_recv().is_err(), "config.check 失败不能继续 apply 或删除数据库记录");
    }

    #[tokio::test]
    async fn user_delete_keeps_disabled_user_when_config_apply_fails() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let user_id = create_proxy_user(&app, proxy_id);
        let current = current_config_with_user(&app, node_id);
        let mut rx = add_agent(&app, node_id, SESSION);
        let delete_app = app.clone();
        let deletion = tokio::spawn(async move {
            crate::proxy_user::delete_proxy_user(Admin, State(delete_app), Path(user_id)).await
        });
        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        reply(&app, node_id, SESSION, &check, Ok(json!({ "valid": true })));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        reply(&app, node_id, SESSION, &apply, Err(json!({ "code": -32000, "message": PRIVATE_SENTINEL })));

        let response = deletion.await.unwrap();
        let body = response_json(response).await;
        assert_eq!(body["deleted"], false);
        assert_eq!(body["user"]["enabled"], false);
        assert_eq!(app.db.proxy_user(user_id).unwrap().unwrap().proxy_node_ids, [proxy_id]);
    }

    #[tokio::test]
    async fn user_delete_keeps_disabled_user_when_agent_is_offline() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let user_id = create_proxy_user(&app, proxy_id);
        let response = crate::proxy_user::delete_proxy_user(Admin, State(app.clone()), Path(user_id)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["deleted"], false);
        assert_eq!(body["user"]["enabled"], false);
        assert_eq!(body["failed_servers"][0]["error"], "服务器当前离线");
        assert_eq!(app.db.proxy_user(user_id).unwrap().unwrap().proxy_node_ids, [proxy_id]);
    }

    #[tokio::test]
    async fn user_update_deploys_every_server_and_keeps_desired_state_on_partial_failure() {
        let app = Arc::new(App::for_test(Db::open(":memory:").unwrap()));
        let first_server = app
            .db
            .create_node(&Node { name: "offline-server".into(), ..Default::default() }, "offline-token")
            .unwrap();
        let second_server = app
            .db
            .create_node(&Node { name: "online-server".into(), ..Default::default() }, "online-token")
            .unwrap();
        let first_proxy = create_proxy_node(&app, first_server, 24443, true, "www.apple.com:443");
        let second_proxy = create_proxy_node(&app, second_server, 24444, true, "www.apple.com:443");
        let user_id = app
            .db
            .create_proxy_user_with_nodes(
                "Alice",
                "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
                true,
                "",
                &[first_proxy, second_proxy],
            )
            .unwrap()
            .0
            .id;
        let second_current = current_config_with_user(&app, second_server);
        let mut rx = add_agent(&app, second_server, SESSION);
        let update_app = app.clone();
        let update = tokio::spawn(async move {
            crate::proxy_user::update_proxy_user(
                Admin,
                State(update_app),
                Path(user_id),
                Ok(Json(crate::proxy_user::UpdateProxyUserRequest {
                    name: "Alice".into(),
                    enabled: false,
                    note: "disabled".into(),
                    proxy_node_ids: vec![first_proxy, second_proxy],
                })),
            )
            .await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, second_server, SESSION, &get, get_result(&second_current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let candidate = check["params"]["content"].as_str().unwrap().to_owned();
        let desired: Value = serde_json::from_str(&candidate).unwrap();
        assert_eq!(desired["inbounds"][0]["users"].as_array().unwrap().len(), 1);
        reply(&app, second_server, SESSION, &check, Ok(json!({ "valid": true })));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        assert_eq!(apply["params"]["content"], candidate);
        reply(&app, second_server, SESSION, &apply, Ok(json!({ "running": true })));

        let response = update.await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["user"]["enabled"], false);
        assert_eq!(body["failed_servers"][0]["server_id"], first_server);
        assert_eq!(body["failed_servers"][0]["error"], "服务器当前离线");
        assert!(!app.db.proxy_user(user_id).unwrap().unwrap().enabled);
        assert_eq!(status(&app, first_proxy).deploy_status, "failed");
        assert_eq!(status(&app, second_proxy).deploy_status, "deployed");
    }

    #[tokio::test]
    async fn regenerate_user_uuid_deploys_the_new_credential() {
        let (app, node_id) = app_node();
        let proxy_id = create_proxy_node(&app, node_id, 443, true, "www.apple.com:443");
        let user_id = create_proxy_user(&app, proxy_id);
        let old_uuid = app.db.proxy_user(user_id).unwrap().unwrap().uuid;
        let current = current_config_with_user(&app, node_id);
        let mut rx = add_agent(&app, node_id, SESSION);
        let regenerate_app = app.clone();
        let regeneration = tokio::spawn(async move {
            crate::proxy_user::regenerate_proxy_user(Admin, State(regenerate_app), Path(user_id)).await
        });

        let get = next_request(&mut rx, SINGBOX_CONFIG_GET_METHOD).await;
        reply(&app, node_id, SESSION, &get, get_result(&current));
        let check = next_request(&mut rx, SINGBOX_CONFIG_CHECK_METHOD).await;
        let candidate = check["params"]["content"].as_str().unwrap().to_owned();
        let config: Value = serde_json::from_str(&candidate).unwrap();
        let user_credential = &config["inbounds"][0]["users"][1];
        let new_uuid = user_credential["uuid"].as_str().unwrap();
        assert_ne!(new_uuid, old_uuid);
        assert_eq!(app.db.proxy_user(user_id).unwrap().unwrap().uuid, new_uuid);
        reply(&app, node_id, SESSION, &check, Ok(json!({ "valid": true })));
        let apply = next_request(&mut rx, SINGBOX_CONFIG_APPLY_METHOD).await;
        assert_eq!(apply["params"]["content"], candidate);
        reply(&app, node_id, SESSION, &apply, Ok(json!({ "running": true })));

        let response = regeneration.await.unwrap();
        let body = response_json(response).await;
        assert_eq!(body["user"]["uuid"], new_uuid);
        assert!(body["failed_servers"].as_array().unwrap().is_empty());
        assert_eq!(status(&app, proxy_id).deploy_status, "deployed");
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
