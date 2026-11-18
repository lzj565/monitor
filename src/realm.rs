//! Reconcile only the newest desired configuration; one command per host in flight.
use crate::{db::RealmDesired, App, Shared};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
struct Flight {
    id: String,
    desired: RealmDesired,
    applying: bool,
    restarting: bool,
}
struct Slot {
    flight: Option<Flight>,
    next: Instant,
    request: i64,
}
fn enqueue(app: &App, node: i64, action: &str, params: Value) -> Result<String, String> {
    let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
    let agent = agents.get(&node).ok_or("节点离线，等待重连")?;
    if !agent.realm_capable {
        return Err("需要升级 Agent 以支持 Realm 管理".into());
    }
    let id = app.commands.start(node, agent.session, action).map_err(|_| "命令队列已满")?;
    if agent
        .tx
        .try_send(json!({"type":"command","id":id,"action":action,"params":params}).to_string())
        .is_err()
    {
        app.commands.cancel(&id, node, agent.session);
        return Err("Agent 命令队列不可用".into());
    }
    Ok(id)
}
fn tick(app: &App, slots: &mut HashMap<i64, Slot>) -> anyhow::Result<()> {
    let hosts = app.db.realm_hosts()?;
    let now = Instant::now();
    let ids: Vec<_> = hosts.iter().filter_map(|h| h["node_id"].as_i64()).collect();
    slots.retain(|id, _| ids.contains(id));
    for node in ids {
        let desired = app.db.realm_desired(node)?;
        let slot = slots.entry(node).or_insert_with(|| Slot { flight: None, next: now, request: -1 });
        if slot.flight.is_none() && slot.request != desired.request {
            slot.request = desired.request;
            slot.next = now;
        }
        if let Some(flight) = slot.flight.take() {
            let command = app.commands.get(&flight.id, node);
            if command.as_ref().is_some_and(|c| c.status == "pending") {
                slot.flight = Some(flight);
                continue;
            }
            slot.next = now + Duration::from_secs(if flight.applying { 0 } else { 15 });
            if let Some(command) = command {
                if command.status == "succeeded" {
                    let observed = command.result.unwrap_or(json!({}));
                    app.db.realm_observed(node, &observed, "")?;
                    let revision = observed["revision"].as_i64().unwrap_or(0);
                    let digest = observed["digest"].as_str().unwrap_or("");
                    let running = observed["running"].as_bool().unwrap_or(false);
                    let enabled = observed["enabled"].as_bool().unwrap_or(false);
                    if revision == flight.desired.revision
                        && digest == flight.desired.digest
                        && running == flight.desired.run
                        && enabled == flight.desired.run
                    {
                        app.db.realm_applied(&flight.desired)?;
                        if flight.desired.restart {
                            app.db.realm_restart_confirm(node, flight.desired.request)?;
                        }
                    }
                    if flight.restarting {
                        app.db.realm_restart_confirm(node, flight.desired.request)?;
                    }
                    if revision > desired.revision
                        || (revision == desired.revision && !digest.is_empty() && digest != desired.digest)
                    {
                        app.db.realm_rebase(node, revision)?;
                        slot.next = now;
                        continue;
                    }
                    let traffic_capable = app
                        .agents
                        .read()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&node)
                        .is_some_and(|a| a.realm_traffic_capable);
                    if traffic_capable
                        && revision == desired.revision
                        && digest == desired.digest
                        && running == desired.run
                        && enabled == desired.run
                        && (observed["traffic"]["digest"] != desired.accounting["digest"]
                            || observed["traffic"]["recheck"] != desired.accounting["recheck"])
                    {
                        match enqueue(
                            app,
                            node,
                            "realm.traffic.configure",
                            json!({"revision":desired.revision,"config_digest":desired.digest,"accounting":desired.accounting}),
                        ) {
                            Ok(id) => {
                                slot.flight = Some(Flight { id, desired, applying: true, restarting: false })
                            }
                            Err(e) => app.db.realm_error(node, &e)?,
                        }
                        continue;
                    }
                    if !flight.applying && !flight.restarting {
                        if observed["installed"] != true || observed["managed"] != true {
                            app.db.realm_observed(node, &observed, "请先安装本安装器管理的 Realm")?;
                            continue;
                        }
                        let action = if desired.restart && running && desired.run && digest == desired.digest
                        {
                            "realm.restart"
                        } else {
                            "realm.config.apply"
                        };
                        if revision != desired.revision
                            || digest != desired.digest
                            || running != desired.run
                            || enabled != desired.run
                            || desired.restart
                        {
                            let params = if action == "realm.restart" {
                                json!({})
                            } else {
                                json!({"revision":desired.revision,"digest":desired.digest,"run":desired.run,"config":desired.config})
                            };
                            match enqueue(app, node, action, params) {
                                Ok(id) => {
                                    slot.flight = Some(Flight {
                                        id,
                                        desired,
                                        applying: action == "realm.config.apply",
                                        restarting: action == "realm.restart",
                                    })
                                }
                                Err(error) => app.db.realm_observed(node, &observed, &error)?,
                            }
                        }
                    }
                } else {
                    let error =
                        command.error.map(|e| e.message).unwrap_or_else(|| "Realm command failed".into());
                    app.db.realm_error(node, &error)?;
                }
            } else {
                app.db.realm_error(node, "Realm 命令超时，稍后重试")?;
            }
        } else if now >= slot.next {
            match enqueue(app, node, "realm.status", json!({})) {
                Ok(id) => slot.flight = Some(Flight { id, desired, applying: false, restarting: false }),
                Err(error) => {
                    app.db.realm_error(node, &error)?;
                    slot.next = now + Duration::from_secs(5);
                }
            }
        }
    }
    Ok(())
}
pub async fn reconcile(app: Shared) {
    let mut slots = HashMap::new();
    loop {
        let state = app.clone();
        let outcome = crate::blocking::run(move || {
            let result = tick(&state, &mut slots);
            (slots, result)
        })
        .await;
        match outcome {
            Ok((next, result)) => {
                slots = next;
                if let Err(error) = result {
                    tracing::warn!("Realm sync failed: {error:#}");
                }
            }
            Err(error) => {
                slots = HashMap::new();
                tracing::warn!("Realm sync worker failed: {error:#}");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
