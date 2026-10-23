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
                host["online"] = json!(agent.is_some());
                host["capable"] = json!(agent.is_some_and(|a| a.realm_capable));
            }
            Json(json!({"items":hosts})).into_response()
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
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
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
        match app
            .db
            .realm_save_rule(id, &body)
            .and_then(|id| app.db.realm_desired(body.node_id).map(|desired| (id, desired)))
        {
            Ok((id, desired)) => Json(json!({"id":id,"desired":desired})).into_response(),
            Err(e) => rejected(e),
        }
    })
    .await
}
pub async fn realm_delete_rule(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.realm_delete_rule(id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
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
