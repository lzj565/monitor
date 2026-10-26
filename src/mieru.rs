//! Durable, serialized reconciliation of the single mita instance on each node.
use crate::{api, api::Admin, db::MieruDesired, App, Shared};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};

struct Flight {
    id: String,
    status: bool,
}
pub async fn reconcile(app: Shared) {
    let mut flights: HashMap<i64, Flight> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let app = app.clone();
        let result = crate::blocking::run(move || {
            let result = step(&app, &mut flights);
            (flights, result)
        })
        .await;
        match result {
            Ok((next, result)) => {
                flights = next;
                if let Err(e) = result {
                    tracing::warn!("mieru reconciliation failed: {e:#}")
                }
            }
            Err(e) => {
                flights = HashMap::new();
                tracing::warn!("mieru worker failed: {e:#}")
            }
        }
    }
}
fn step(app: &App, flights: &mut HashMap<i64, Flight>) -> anyhow::Result<()> {
    let nodes: std::collections::BTreeSet<_> = app
        .db
        .all_proxies()?
        .iter()
        .filter(|p| p.protocol == "mieru")
        .map(|p| p.node_id)
        .chain(app.db.mieru_hosts()?.iter().filter_map(|h| h["node_id"].as_i64()))
        .collect();
    flights.retain(|n, _| nodes.contains(n));
    for node in nodes {
        let mut observed = None;
        if let Some(f) = flights.remove(&node) {
            if let Some(command) = app.commands.get(&f.id, node) {
                if command.status == "pending" {
                    flights.insert(node, f);
                    continue;
                }
                if command.status == "succeeded" {
                    let result = command.result.unwrap_or(json!({}));
                    app.db.mieru_observed(node, &result, "")?;
                    if f.status {
                        observed = Some(result)
                    }
                } else {
                    app.db.mieru_observed(
                        node,
                        &json!({}),
                        command
                            .error
                            .as_ref()
                            .map(|e| e.message.as_str())
                            .unwrap_or("配置应用失败，等待重试"),
                    )?;
                }
            }
        }
        let Some(mut desired) = app.db.mieru_desired(node)? else { continue };
        let status = if let Some(v) = observed {
            if let Some(rev) = v["revision"].as_i64().filter(|r| *r > desired.revision) {
                app.db.mieru_rebase(node, rev)?;
                desired = app.db.mieru_desired(node)?.unwrap();
            }
            let run = serde_json::from_str::<Value>(&desired.content)?["run"].as_bool().unwrap_or(false);
            if v["revision"] == desired.revision
                && v["digest"] == desired.digest
                && v["running"] == run
                && v["enabled"] == run
            {
                continue;
            }
            false
        } else {
            true
        };
        match enqueue(app, &desired, status) {
            Ok(id) => {
                flights.insert(node, Flight { id, status });
            }
            Err(e) => app.db.mieru_observed(node, &json!({}), &e)?,
        }
    }
    Ok(())
}
fn enqueue(app: &App, d: &MieruDesired, status: bool) -> Result<String, String> {
    let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
    let a = agents.get(&d.node_id).ok_or("节点离线，等待重连")?;
    if !a.mieru_capable {
        return Err("需要升级 Agent 以支持 mieru".into());
    }
    let action = if status { "mieru.status" } else { "mieru.config.apply" };
    let id = app.commands.start(d.node_id, a.session, action).map_err(|_| "命令队列已满")?;
    let msg=json!({"type":"command","id":id,"action":action,"params":if status{json!({})}else{json!({"revision":d.revision,"digest":d.digest,"content":d.content})}}).to_string();
    if a.tx.try_send(msg).is_err() {
        app.commands.cancel(&id, d.node_id, a.session);
        return Err("Agent 队列不可用".into());
    }
    Ok(id)
}
pub async fn hosts(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.mieru_hosts() {
        Ok(mut items) => {
            let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
            for h in &mut items {
                let a = h["node_id"].as_i64().and_then(|id| agents.get(&id));
                h["online"] = json!(a.is_some());
                h["capable"] = json!(a.is_some_and(|a| a.mieru_capable));
            }
            Json(json!({"items":items})).into_response()
        }
        Err(_) => api::answer(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "读取 mieru 状态失败"),
    })
    .await
}
pub async fn preview(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.mieru_desired(id) {
        Ok(Some(d)) => ([(axum::http::header::CACHE_CONTROL,"no-store")],Json(json!({"revision":d.revision,"digest":d.digest,"content":serde_json::from_str::<Value>(&d.content).unwrap_or(Value::Null)}))).into_response(),
        Ok(None) => api::answer(axum::http::StatusCode::NOT_FOUND,"mieru 配置不存在"),
        Err(e) => api::fail(e),
    }).await
}
pub async fn reset_credential(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.mieru_password(true) {
        Ok(password) => {
            ([(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({"password":password})))
                .into_response()
        }
        Err(_) => api::answer(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "读取 mieru 全局密码失败"),
    })
    .await
}
pub async fn asset(State(app): State<Shared>, Path(arch): Path<String>) -> Response {
    let arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return api::answer(axum::http::StatusCode::NOT_FOUND, "未知架构"),
    };
    let url = crate::proxied(
        &app,
        format!("https://github.com/enfein/mieru/releases/download/v3.39.0/mita_3.39.0_linux_{arch}.tar.gz"),
    );
    // Share the existing global relay concurrency and transfer deadline.
    let Ok(Ok(permit)) = tokio::time::timeout(crate::RELAY_WAIT, crate::RELAY_GATE.acquire()).await else {
        return api::answer(axum::http::StatusCode::SERVICE_UNAVAILABLE, "下载繁忙");
    };
    match app.http.get(url).send().await {
        Ok(r) if r.status().is_success() => (
            [(axum::http::header::CONTENT_TYPE, "application/gzip")],
            axum::body::Body::from_stream(crate::metered(Box::pin(r.bytes_stream()), permit)),
        )
            .into_response(),
        _ => api::answer(axum::http::StatusCode::BAD_GATEWAY, "mita 下载失败"),
    }
}

pub async fn credential(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.mieru_password(false) {
        Ok(password) => {
            ([(axum::http::header::CACHE_CONTROL, "no-store")], Json(json!({"password":password})))
                .into_response()
        }
        Err(_) => api::answer(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "读取 mieru 全局密码失败"),
    })
    .await
}
