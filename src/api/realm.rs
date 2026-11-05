use super::*;
fn rejected(error: anyhow::Error) -> Response {
    if error.downcast_ref::<crate::Shown>().is_some() {
        api_error(StatusCode::CONFLICT, "REALM_CONFIG_INVALID", error.to_string())
    } else {
        api_internal(error)
    }
}
pub async fn realm_hosts(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.realm_hosts() {
        Ok(mut hosts) => {
            let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
            for host in &mut hosts {
                let agent = host["node_id"].as_i64().and_then(|id| agents.get(&id));
                host["traffic_capable"] = json!(agent.is_some_and(|a| a.realm_traffic_capable));
                host["online"] = json!(agent.is_some());
                host["capable"] = json!(agent.is_some_and(|a| a.realm_capable));
            }
            let candidates = app.db.realm_entry_candidates();
            match candidates {
                Ok(ids) => Json(json!({"items":hosts,"entry_candidates":ids})).into_response(),
                Err(e) => fail(e),
            }
        }
        Err(e) => fail(e),
    })
    .await
}
pub async fn realm_save_host(
    _: Admin,
    State(app): State<Shared>,
    Json(body): Json<crate::db::RealmHostInput>,
) -> Response {
    crate::blocking::response(move || {
        match app.db.realm_save_host(&body).and_then(|()| app.db.realm_desired(body.node_id)) {
            Ok(want) => Json(json!({"desired":want})).into_response(),
            Err(e) => rejected(e),
        }
    })
    .await
}
pub async fn realm_delete_host(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.realm_delete_host(id) {
        Ok(true) => {
            let nodes = app
                .db
                .all_proxies()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.node_id)
                .collect::<std::collections::BTreeSet<_>>();
            sync_generated_nodes(&app, nodes.into_iter().collect());
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => rejected(e),
    })
    .await
}
pub async fn realm_rules(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.realm_rules() {
        Ok(items) => Json(json!({"items":items})).into_response(),
        Err(e) => fail(e),
    })
    .await
}
pub async fn realm_create_rule(
    _: Admin,
    State(app): State<Shared>,
    Json(body): Json<crate::db::RealmRuleInput>,
) -> Response {
    save_rule(app, None, body).await
}
pub async fn realm_update_rule(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<crate::db::RealmRuleInput>,
) -> Response {
    save_rule(app, Some(id), body).await
}
async fn save_rule(app: Shared, id: Option<i64>, body: crate::db::RealmRuleInput) -> Response {
    crate::blocking::response(move || {
        let unchanged = id.is_some_and(|id| {
            app.db
                .realm_rules()
                .is_ok_and(|rules| rules.iter().any(|r| r["id"] == id && r["node_id"] == body.node_id))
        });
        if !unchanged && !app.db.realm_entry_candidates().is_ok_and(|ids| ids.contains(&body.node_id)) {
            return api_error(
                StatusCode::CONFLICT,
                "REALM_NOT_INSTALLED",
                "入口节点尚未确认安装 Realm，请安装后刷新状态",
            );
        }

        match app
            .db
            .realm_save_rule(id, &body)
            .and_then(|id| app.db.realm_desired(body.node_id).map(|desired| (id, desired)))
        {
            Ok((id, desired)) => {
                let nodes = app
                    .db
                    .all_proxies()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| p.node_id)
                    .collect::<std::collections::BTreeSet<_>>();
                let sync = sync_generated_nodes(&app, nodes.into_iter().collect());
                Json(json!({"id":id,"desired":desired,"sync":sync})).into_response()
            }
            Err(e) => rejected(e),
        }
    })
    .await
}
pub async fn realm_delete_rule(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.realm_delete_rule(id) {
        Ok(true) => {
            let nodes = app
                .db
                .all_proxies()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.node_id)
                .collect::<std::collections::BTreeSet<_>>();
            sync_generated_nodes(&app, nodes.into_iter().collect());
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => rejected(e),
    })
    .await
}
pub async fn realm_preview(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.realm_desired(id) {
        Ok(desired) => Json(json!({"desired":desired})).into_response(),
        Err(e) => rejected(e),
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmOperation {
    action: String,
}
pub async fn realm_operation(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<RealmOperation>,
) -> Response {
    crate::blocking::response(move || {
        if body.action == "traffic_recheck" {
            return match app.db.realm_traffic_recheck(id) {
                Ok(()) => Json(json!({"queued":true})).into_response(),
                Err(e) => rejected(e),
            };
        }
        if !["refresh", "retry", "restart"].contains(&body.action.as_str()) {
            return bad("不支持的 Realm 操作");
        }
        match app.db.realm_request(id, body.action == "restart") {
            Ok(()) => Json(json!({"queued":true})).into_response(),
            Err(e) => rejected(e),
        }
    })
    .await
}

pub async fn realm_traffic(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.realm_traffic() {
        Ok(mut v) => {
            let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
            if let Some(hosts) = v["hosts"].as_array_mut() {
                for h in hosts {
                    if !h["node_id"].as_i64().is_some_and(|id| agents.contains_key(&id)) {
                        h["status"]["fresh"] = json!(false);
                    }
                }
            }
            if let Some(rules) = v["rules"].as_array_mut() {
                for r in rules {
                    if !r["node_id"].as_i64().is_some_and(|id| agents.contains_key(&id)) {
                        r["up_rate"] = serde_json::Value::Null;
                        r["down_rate"] = serde_json::Value::Null;
                    }
                }
            }
            Json(v).into_response()
        }
        Err(e) => fail(e),
    })
    .await
}
