use super::*;

pub async fn create_node(
    _: Admin,
    State(app): State<Shared>,
    headers: HeaderMap,
    body: Result<Json<Node>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        if let Err(refusal) = provisioning_allowed(&app, &headers) {
            return answer(StatusCode::FORBIDDEN, refusal);
        }
        let Ok(Json(mut node)) = body else { return bad("节点数据格式不对") };
        if let Some(message) = name_error(&mut node.name)
            .or_else(|| node_limits(Some(node.traffic_reset_day), Some(node.price), Some(node.traffic_limit)))
            .or_else(|| group_error(&mut node.group))
            .or_else(|| billing_error(Some(&mut node.currency), Some(&mut node.billing_cycle)))
        {
            return bad(message);
        }
        let token = random_token();
        match app.db.create_node(&node, &token) {
            // Usable immediately: the install dialog reads the token as it opens, so
            // adding and deploying require no reissue in between.
            Ok(id) => {
                invalidate_snapshot(&app);
                Json(json!({"id": id})).into_response()
            }
            Err(e) => fail(e),
        }
    })
    .await
}

// ---- automatic registration ----

/// How long a registration window stays open.
///
/// Provisioning a batch of machines takes minutes, and the window expires on its
/// own rather than depending on someone returning to close it.
pub(super) const REGISTER_WINDOW: i64 = 3600;

/// How many nodes one window may register.
///
/// Without it, whoever holds the key for the hour could fill the node table. A
/// hundred is well beyond a plausible batch and well short of a problem.
pub(super) const REGISTER_LIMIT: i64 = 100;

/// The header in which `install.sh` sends the token the machine already holds.
pub(super) const HELD_TOKEN: &str = "x-node-token";

/// Exchanges a registration key for a node token, so a batch of machines can be
/// installed with one command rather than one panel visit each.
///
/// No session stands behind this route: the caller is `install.sh` on a machine
/// that has never contacted the hub. A key issued by the panel, valid only within
/// [`REGISTER_WINDOW`], serves in place of a session.
///
/// `provisioning_allowed` does not guard it, because a shell script sends no
/// `Origin` and the proxy headers that remain cannot state what address the
/// caller used. Which addresses may be registered against is enforced in
/// `install.sh`, where that address is known: it refuses plaintext to anything
/// but loopback unless `--insecure` is given, the same switch under which the
/// binary it is about to run as root was already fetched in the clear. The
/// window this key belongs to is still opened from an https domain entry alone.
///
/// One request costs at most a lookup on the token's unique index, two setting
/// reads, a `COUNT`, and one transaction inserting the `node` and `traffic` rows
/// plus a `ping_node` row per `auto_join` probe, at most 64. It makes no outbound
/// request, and the router's 64 KiB body limit bounds the name. Requests in flight
/// are not gated: each step is a point read or one small transaction under the
/// database lock, as with the token lookup of the agent handshake, and an address
/// locked out below reaches none of them. 120 concurrent callers with the window
/// closed move the panel's median from 1.1 ms to 2.1-2.8 ms, with or without a
/// held token, on three cores.
pub async fn agent_register(
    State(app): State<Shared>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    // Plain text in, plain text out. The caller is a shell script, as with this
    // route's neighbours: `/install.sh` and `/agent/{arch}` return a script and a
    // binary. A bare token is one `$(curl ...)` away, requiring no JSON parser in
    // a POSIX `sh`.
    name: String,
) -> Response {
    let ip = client_ip(&app, &headers, peer.ip());
    // Counted separately from the sign-in page: a batch install started with a
    // stale key is a misconfigured deploy rather than an attack on the panel, and
    // a shared counter would lock the operator out of their own hub for LOCKOUT.
    if app.registrations.locked(ip) {
        return answer(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try again later");
    }
    // A rerun on a registered machine sends the token it already holds and
    // receives it back while that token still opens a node, so the rerun adds no
    // second node. Ahead of the window and the key: it creates nothing, returns
    // only what the caller already holds -- which the 401 of the agent handshake
    // reveals as well -- and must still succeed once the window has closed. A
    // token whose node was deleted, or which was reissued, falls through.
    if let Some(held) = headers.get(HELD_TOKEN).and_then(|v| v.to_str().ok()).filter(|t| !t.is_empty()) {
        match app.db.node_by_token(held) {
            Ok(Some(_)) => return held.to_owned().into_response(),
            Ok(None) => {}
            // Read as "no node", a failed lookup would register a second node for
            // a machine whose node is intact.
            Err(e) => return script_fail(e),
        }
    }
    // One answer for both "no window is open" and "that key is wrong": the
    // difference is only useful to someone who has neither.
    let closed = || {
        answer(StatusCode::FORBIDDEN, "registration is closed; open a new window from the panel's node list")
    };
    let until = register_until(&app);
    let Some(key) = app.db.get("register_key").filter(|k| !k.is_empty() && Utc::now().timestamp() < until)
    else {
        return closed();
    };
    if agent_ws::bearer(&headers) != Some(key.as_str()) {
        // Only an incorrect key counts against the address. With the window
        // closed there is no secret to guess, and counting then would let anyone
        // lock an address they name in `X-Forwarded-For` out of the sign-in page.
        app.registrations.record_failure(ip);
        return closed();
    }
    match app.db.nodes_created_since(until - REGISTER_WINDOW) {
        Ok(n) if n >= REGISTER_LIMIT => {
            return answer(
                StatusCode::FORBIDDEN,
                "this window has registered enough nodes; open a new window from the panel's node list",
            )
        }
        Err(e) => return script_fail(e),
        Ok(_) => {}
    }

    // The name comes from a machine not yet vouched for, and is shown to every
    // visitor until the operator renames the node. Trimmed last: a name made of
    // markup characters and spaces must come out empty, not as a blank, and
    // `create_node` names an empty one after the node's id.
    let name = db::reported_text(&name, MAX_NAME).trim().to_owned();
    // Field defaults live in `Node`'s serde attributes and nowhere else.
    // `Node::default()` is a different set of values -- private, reset day 0 --
    // and a node registered here must match one added through the panel.
    let node = match serde_json::from_value::<Node>(json!({ "name": name })) {
        Ok(node) => node,
        Err(e) => return script_fail(e),
    };
    let flag = |key: &str| -> Result<bool, &'static str> {
        match headers.get(key).map(|h| h.to_str()) {
            None | Some(Ok("0")) => Ok(false),
            Some(Ok("1")) => Ok(true),
            _ => Err("invalid component registration option"),
        }
    };
    let realm = match flag("x-install-realm") {
        Ok(v) => v,
        Err(r) => return bad(r),
    };
    let mieru = match flag("x-install-mieru") {
        Ok(v) => v,
        Err(r) => return bad(r),
    };
    let token = random_token();
    match app.db.create_node_with_components(
        &node,
        &token,
        &crate::db::InstallChoices { singbox: true, realm, mieru },
    ) {
        Ok(_) => {
            app.registrations.clear(ip);
            invalidate_snapshot(&app);
            token.into_response()
        }
        Err(e) => script_fail(e),
    }
}

/// [`fail`] for `install.sh`, whose own output is English and which prints the
/// reply in one line after its own.
pub(super) fn script_fail(e: impl Into<anyhow::Error>) -> Response {
    warn!("registration failed: {:#}", e.into());
    answer(StatusCode::INTERNAL_SERVER_ERROR, "the hub hit an internal error; its log has the details")
}

/// Opens a registration window with a fresh key. Any previous key stops working
/// the moment this returns.
pub async fn open_register(_: Admin, State(app): State<Shared>, headers: HeaderMap) -> Response {
    crate::blocking::response(move || {
        if let Err(refusal) = provisioning_allowed(&app, &headers) {
            return answer(StatusCode::FORBIDDEN, refusal);
        }
        let key = random_token();
        let until = (Utc::now().timestamp() + REGISTER_WINDOW).to_string();
        match app.db.set("register_key", &key).and_then(|()| app.db.set("register_until", &until)) {
            Ok(()) => Json(json!({"register_key": key, "register_left": REGISTER_WINDOW})).into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}

/// Closes the window early, before the hour elapses.
pub async fn close_register(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || {
        match app.db.set("register_key", "").and_then(|()| app.db.set("register_until", "0")) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}

pub async fn update_node(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<NodePatch>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let Ok(Json(mut node)) = body else { return bad("节点数据格式不对") };
        if let Some(message) = patch_error(&mut node) {
            return bad(message);
        }
        match app.db.update_node(id, &node) {
            Ok(true) => {
                invalidate_snapshot(&app);
                Json(json!({"ok": true})).into_response()
            }
            Ok(false) => no_such_node(),
            Err(e) => fail(e),
        }
    })
    .await
}

/// What a batch may set: the settings the panel applies across a selection.
/// An allowlist, so a field added to [`NodePatch`] later, possibly one that
/// describes a single machine, is refused here until it is listed.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BatchPatch {
    pub(super) group: Option<String>,
    pub(super) notify: Option<bool>,
}

#[derive(Deserialize)]
pub struct NodeBatch {
    pub(super) ids: Vec<i64>,
    #[serde(default)]
    pub(super) patch: BatchPatch,
}

/// Applies one patch to every selected node, all or none.
pub async fn update_nodes(
    _: Admin,
    State(app): State<Shared>,
    body: Result<Json<NodeBatch>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let Ok(Json(NodeBatch { mut ids, patch })) = body else {
            return bad("批量修改只支持分组和通知");
        };
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return bad("没有选中节点");
        }
        let mut patch = NodePatch { group: patch.group, notify: patch.notify, ..Default::default() };
        if let Some(message) = patch_error(&mut patch) {
            return bad(message);
        }
        match app.db.update_nodes(&ids, &patch) {
            Ok(true) => {
                invalidate_snapshot(&app);
                Json(json!({"updated": ids.len()})).into_response()
            }
            Ok(false) => answer(StatusCode::NOT_FOUND, "有节点已被删除，没有做任何修改；刷新后重新选择"),
            Err(e) => fail(e),
        }
    })
    .await
}

#[derive(Deserialize)]
pub struct Order {
    pub(super) ids: Vec<i64>,
}

/// The list must name every node exactly once, checked inside the transaction
/// that renumbers rather than here: re-reading the node list first would only
/// race the write it guards.
pub async fn reorder_nodes(_: Admin, State(app): State<Shared>, Json(order): Json<Order>) -> Response {
    crate::blocking::response(move || match app.db.reorder_nodes(&order.ids) {
        Ok(()) => {
            invalidate_snapshot(&app);
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => fail(e),
    })
    .await
}

pub async fn reorder_proxies(_: Admin, State(app): State<Shared>, Json(order): Json<Order>) -> Response {
    crate::blocking::response(move || match app.db.reorder_proxies(&order.ids) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => fail(e),
    })
    .await
}

pub async fn delete_node(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || {
        match app.db.realm_refs(Some(id),None) {
            Ok(refs) if !refs.is_empty()=>return (StatusCode::CONFLICT,Json(json!({"error":{"code":"REALM_REFERENCED","message":"节点仍被 Realm 引用","references":refs}}))).into_response(),
            Err(e)=>return fail(e), _=>{}
        }
        match app.db.delete_node(id) {
            Ok(true) => {}
            Ok(false) => return no_such_node(),
            Err(e) => return fail(e),
        }
        // The token is checked only at the handshake, so deleting the row does not
        // end a connection already open on it; dropping the sender does. Dropped
        // after the delete, so the reconnect that follows finds no token to accept.
        // The same reasoning applies in `reset_token` below.
        app.agents.write().unwrap_or_else(|e| e.into_inner()).remove(&id);
        // Its held reading as well, which no node would book.
        app.readings.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        invalidate_snapshot(&app);
        Json(json!({"ok": true})).into_response()
    })
    .await
}

/// Issues a fresh token, invalidating the old one immediately.
///
/// Always an explicit action: rotate a token believed to have leaked, then
/// reinstall the agent. Reading the install command does not pass through here.
pub async fn reset_token(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || {
        let token = random_token();
        match app.db.reset_token(id, &token) {
            Ok(true) => {}
            Ok(false) => return no_such_node(),
            Err(e) => return fail(e),
        }
        // The token is checked only at the handshake, so a session opened with the
        // old one would continue reporting. Dropping the sender ends that loop; the
        // agent reconnects and is refused. Its own teardown leaves the entry
        // untouched, because the session tag no longer matches.
        app.agents.write().unwrap_or_else(|e| e.into_inner()).remove(&id);
        // The node shows offline at once rather than when the frame expires.
        invalidate_snapshot(&app);
        // The token alone: the panel builds the command, and one place needs to know
        // its form.
        Json(json!({"token": token})).into_response()
    })
    .await
}

pub async fn patch_traffic(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    Json(p): Json<TrafficPatch>,
) -> Response {
    crate::blocking::response(move || {
        if [p.total_rx, p.total_tx, p.month_rx, p.month_tx].into_iter().flatten().any(|v| v < 0) {
            return bad("流量不能是负数");
        }
        match app.db.set_traffic(id, &p) {
            Ok(true) => {
                invalidate_snapshot(&app);
                Json(json!({"ok": true})).into_response()
            }
            Ok(false) => no_such_node(),
            Err(e) => fail(e),
        }
    })
    .await
}

pub async fn ping_tasks(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.ping_tasks() {
        Ok(tasks) => Json(json!({"tasks": tasks})).into_response(),
        Err(e) => fail(e),
    })
    .await
}

/// As `reorder_nodes`. Nothing is pushed to the agents: the list they run is in
/// id order, see `ping_tasks_for`.
pub async fn reorder_ping_tasks(_: Admin, State(app): State<Shared>, Json(order): Json<Order>) -> Response {
    crate::blocking::response(move || match app.db.reorder_ping_tasks(&order.ids) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => fail(e),
    })
    .await
}

/// A probe target the agent can resolve: `host:port`, with an IPv6 literal
/// bracketed as a URL writes one.
///
/// A bare `contains(':')` admitted three forms that never connect: a bare IPv6
/// address, which is all colons; `:443` with no host; and `host:` with no port.
/// The agent's `lookup_host` errors on each, `tcp_ping` returns -1, and the chart
/// draws a probe at 100% loss indefinitely with nothing in any log identifying
/// the target as the cause.
pub(super) fn valid_target(target: &str) -> bool {
    let (host, port) = match target.strip_prefix('[') {
        Some(rest) => match rest.split_once("]:") {
            Some(pair) => pair,
            None => return false,
        },
        // Unbracketed, so the last colon is the port separator; anything still
        // containing a colon is an IPv6 address that required brackets.
        None => match target.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, port),
            _ => return false,
        },
    };
    !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p > 0)
}

pub async fn save_ping_task(_: Admin, State(app): State<Shared>, Json(mut task): Json<PingTask>) -> Response {
    crate::blocking::response(move || {
        // Trimmed into the stored value rather than a discarded copy: what reaches
        // the agent is `task.target`, and a trailing space from a paste passes
        // `valid_target` while `lookup_host` rejects the stored string outright,
        // leaving the probe reporting -1 indefinitely. The name is trimmed for the
        // same reason, as it travels to the public page as a chart label.
        task.name = task.name.trim().to_owned();
        task.target = task.target.trim().to_owned();
        if task.name.is_empty() || task.target.is_empty() {
            return bad("请填写名称和目标");
        }
        if task.name.chars().count() > MAX_NAME {
            return bad("名称最多 64 个字");
        }
        // A TCP probe requires an explicit port; a bare host would silently never
        // connect.
        if !valid_target(&task.target) {
            return bad("目标要写成「主机:端口」，例如 1.1.1.1:443 或 [2606:4700:4700::1111]:443");
        }
        // Refused rather than clamped, for the reason `setting_error` gives for
        // `retention_days`: the agent clamps this again on arrival, so an
        // out-of-range value never fails but silently becomes a different number
        // while the panel still displays what was entered. Below the floor that
        // number is 5 seconds, the fastest probe available, run by every node the
        // task is assigned to; the panel reaches 0 simply by having its interval
        // field cleared.
        if !(Db::MIN_PROBE_INTERVAL..=3_600).contains(&task.interval) {
            return bad(&format!("间隔要在 {} 到 3600 秒之间", Db::MIN_PROBE_INTERVAL));
        }
        match app.db.save_ping_task(&task) {
            Ok(id) => {
                agent_ws::push_ping_tasks(&app, None);
                Json(json!({"id": id})).into_response()
            }
            Err(e) => fail(e),
        }
    })
    .await
}

pub async fn delete_ping_task(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.delete_ping_task(id) {
        Ok(()) => {
            agent_ws::push_ping_tasks(&app, None);
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => fail(e),
    })
    .await
}

/// Seconds the registration window has left, 0 when none is open. What the
/// panel counts down from, rather than the stored deadline: compared with the
/// browser's clock, a deadline is off by as much as that clock is. A browser
/// eight hours fast -- Windows and Linux sharing a machine in UTC+8 -- would hide
/// the command of an open window, and one eight hours slow would show it for
/// eight hours after the key stopped working.
pub(super) fn register_left(app: &App) -> i64 {
    (register_until(app) - Utc::now().timestamp()).clamp(0, REGISTER_WINDOW)
}

/// The stored deadline, 0 when none was ever set. One reading for the gate in
/// `agent_register` and the panel's countdown, so the two cannot disagree.
pub(super) fn register_until(app: &App) -> i64 {
    app.db.get("register_until").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)
}

/// A node's token, for the install command the panel shows. Not part of the
/// node list, where it would share an answer with strings agents report: a
/// proxy compressing that answer would let one rogue agent guess it a character
/// at a time from the compressed length.
pub async fn node_token(_: Admin, State(app): State<Shared>, Path(id): Path<i64>) -> Response {
    crate::blocking::response(move || match app.db.node(id) {
        Ok(Some(node)) => {
            ([(header::CACHE_CONTROL, "no-store")], Json(json!({"token": node.token}))).into_response()
        }
        Ok(None) => no_such_node(),
        Err(e) => fail(e),
    })
    .await
}

/// The probes ticked for one node, and `base`, those it had when the editor
/// opened; see `Db::set_node_ping_tasks`.
#[derive(Deserialize)]
pub struct NodeProbes {
    tasks: Vec<i64>,
    base: Vec<i64>,
}

pub async fn set_node_ping_tasks(
    _: Admin,
    State(app): State<Shared>,
    Path(id): Path<i64>,
    body: Result<Json<NodeProbes>, JsonRejection>,
) -> Response {
    crate::blocking::response(move || {
        let Ok(Json(probes)) = body else { return bad("监控列表格式不对") };
        match app.db.set_node_ping_tasks(id, &probes.tasks, &probes.base) {
            Ok(true) => {
                agent_ws::push_ping_tasks(&app, Some(id));
                Json(json!({"ok": true})).into_response()
            }
            Ok(false) => no_such_node(),
            Err(e) => fail(e),
        }
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyListOrder {
    pub ids: Vec<String>,
}

pub async fn proxy_list_order(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || match app.db.proxy_list_order() {
        Ok(ids) => ([(header::CACHE_CONTROL, "no-store")], Json(json!({"ids":ids}))).into_response(),
        Err(e) => fail(e),
    })
    .await
}

pub async fn save_proxy_list_order(
    _: Admin,
    State(app): State<Shared>,
    Json(order): Json<ProxyListOrder>,
) -> Response {
    crate::blocking::response(move || match app.db.save_proxy_list_order(&order.ids) {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(e) => fail(e),
    })
    .await
}
