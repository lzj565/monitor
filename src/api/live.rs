use super::*;

// ---- read paths, shared between the panel and the public page ----

/// Everything a report may expose under `metrics`: the agent contract minus the
/// raw kernel counters, which are a wire-protocol detail disclosing a machine's
/// entire lifetime traffic, plus the four traffic figures `node_view` fills in.
/// The panel additionally sees `iface`.
pub(crate) const PUBLIC_METRICS: [&str; 18] = [
    "uptime",
    "cpu",
    "load",
    "mem_total",
    "mem_used",
    "swap_total",
    "swap_used",
    "disk_total",
    "disk_used",
    "net_rx",
    "net_tx",
    "tcp",
    "udp",
    "procs",
    "total_rx",
    "total_tx",
    "month_rx",
    "month_tx",
];

/// The addresses the panel shows for a node, each with where it comes from: at
/// most one per family, v4 first, the one the machine is reached by. The agent
/// reports its interfaces; `ip` is where its connection arrived from, in dotted
/// form for IPv4.
///
/// Per family an address set by hand comes first, then a public one on the
/// interface. Failing both, where the interface holds only a private address of
/// the family the connection used -- NAT, or a proxy in front -- the
/// connection's public address, its exit, stands in. An exit in a family the
/// interface does not hold is a translator such as NAT64 or WARP and is left
/// out.
///
/// Private addresses appear only when nothing public is known, as where hub and
/// node share a network and they are all there is. `ip` alone is the fallback
/// for an agent reporting no interface.
pub(super) fn addresses<'a>(
    exits: [&'a str; 2],
    (ipv4, ipv6): (&'a str, &'a str),
    (pin4, pin6): (&'a str, &'a str),
) -> Vec<(&'a str, &'static str)> {
    let public =
        |a: &str, v6: bool| a.parse::<IpAddr>().is_ok_and(|a| a.is_ipv6() == v6 && agent_ws::public(a));
    let family = |pin: &'a str, held: &'a str, v6: bool| {
        if !pin.is_empty() {
            Some((pin, "manual"))
        } else if public(held, v6) {
            Some((held, "interface"))
        } else if held.is_empty() {
            None
        } else {
            exits.into_iter().find(|ip| public(ip, v6)).map(|exit| (exit, "exit"))
        }
    };
    let shown: Vec<_> = [family(pin4, ipv4, false), family(pin6, ipv6, true)].into_iter().flatten().collect();
    if !shown.is_empty() {
        return shown;
    }
    let held: Vec<_> = [ipv4, ipv6].into_iter().filter(|a| !a.is_empty()).map(|a| (a, "interface")).collect();
    if !held.is_empty() {
        return held;
    }
    [exits[0]].into_iter().filter(|a| !a.is_empty()).map(|a| (a, "connection")).collect()
}

/// One node as the UI consumes it: stored config, live metrics and the hub's
/// accumulated traffic in a single object.
pub(super) fn node_view(
    node: &Node,
    current: Option<&Agent>,
    traffic: &Traffic,
    full: bool,
    now: DateTime<Local>,
) -> Value {
    // The three capacities arrive twice: once in `Facts`, sent at the handshake,
    // and again in every `Metrics`. The stored figure follows the reports a
    // minute at a time and as the session ends, so it lags a disk mounted while
    // the agent runs -- the agent re-reads its mount table every sample so that
    // it appears -- by up to a minute. Using the report while a node is connected
    // keeps every consumer of this view on one number: the card reads the live
    // metrics and the detail page reads these, which would otherwise show the
    // same machine two different capacities. Offline, the stored figure is the
    // last one reported. No floor is applied: a host whose swap has just been
    // disabled reports zero and means it. A node connected but not yet reporting
    // holds `Null`, where `get` returns nothing and the stored figure stands.
    let live = |key: &str, stored: i64| {
        current.and_then(|a| a.metrics.get(key).and_then(serde_json::Value::as_i64)).unwrap_or(stored)
    };
    // The live entry while connected, the stored one afterwards. Zero means
    // connected but not yet reporting, which is not a timestamp, so it falls
    // back to the stored value and "offline since" survives the gap.
    let last_seen = current.map(|a| a.last_seen).filter(|t| *t > 0).unwrap_or(node.last_seen);
    let mut view = json!({
        "id": node.id,
        "name": node.name,
        // A country rather than an address: it indicates which region a node sits
        // in, which is what a status page conveys, without locating it. The
        // address it was derived from remains behind the panel.
        "country": if node.country_pin.is_empty() { &node.country } else { &node.country_pin },
        // Named by the operator for the status page to divide the list by, so
        // public like the node's name. Empty is ungrouped.
        "group": node.group,
        // Written by the operator for visitors, unlike `remark` below.
        "public_remark": node.public_remark,
        "sort": node.sort,
        "public": node.public,
        "online": current.is_some(),
        "last_seen": last_seen,
        // Counted on the hub's clock, for the same reason as `expires_in`: a page
        // subtracting `last_seen` from its own clock is off by as much as that
        // clock is, and one eight hours fast shows a node that dropped a minute
        // ago as offline for eight hours. Null for a node never seen.
        "last_seen_ago": (last_seen > 0).then(|| (now.timestamp() - last_seen).max(0)),
        "metrics": current.map(|a| a.metrics.clone()).unwrap_or(Value::Null),
        "os": node.os,
        "kernel": node.kernel,
        "arch": node.arch,
        "virt": node.virt,
        "cpu_name": node.cpu_name,
        "cpu_cores": node.cpu_cores,
        "mem_total": live("mem_total", node.mem_total),
        "swap_total": live("swap_total", node.swap_total),
        "disk_total": live("disk_total", node.disk_total),
        "agent_version": node.agent_version,
        "price": node.price,
        "currency": node.currency,
        "billing_cycle": node.billing_cycle,
        "expires_at": node.expires_at,
        // Counted on the hub's calendar, the one renewal follows. A page counting
        // on the visitor's clock would, with the hub on UTC and the visitor on
        // UTC+8, show every online node expired for eight hours each cycle
        // before the hub rolls its date forward.
        "expires_in": node.expires_at.as_deref().and_then(|d| d.parse::<NaiveDate>().ok()).map(|d| (d - now.date_naive()).num_days()),
        "traffic_limit": node.traffic_limit,
        "traffic_mode": node.traffic_mode,
        "traffic_reset_day": node.traffic_reset_day,
        "total_rx": traffic.total_rx,
        "total_tx": traffic.total_tx,
        "month_rx": traffic.month_rx,
        "month_tx": traffic.month_tx,
        "month_used": traffic.month_used(&node.traffic_mode),
        "month_start": traffic.month_start,
        // Of the same nature as the month and lifetime figures beside it, which
        // the public page already shows, so this one is public as well.
        "day_rx": traffic.day_rx,
        "day_tx": traffic.day_tx,
    });
    // An allowlist rather than a denylist, for the panel as well: the agent ships
    // from its own repository, so a field added there would otherwise reach
    // anonymous visitors the day it is released, and a node token in the wrong
    // hands could fill the panel's frame with whatever it sends. No address,
    // hostname or private note may ever reach a visitor.
    if let Some(m) = view["metrics"].as_object_mut() {
        m.retain(|k, _| PUBLIC_METRICS.contains(&k.as_str()) || (full && k == "iface"));
        // The same figures as the top-level ones, from the same row. Both official
        // themes refuse a node's live view without them.
        for (key, value) in agent_ws::INJECTED.into_iter().zip([
            traffic.total_rx,
            traffic.total_tx,
            traffic.month_rx,
            traffic.month_tx,
        ]) {
            m.insert(key.into(), json!(value));
        }
    }
    // Address and private notes never leave the panel. The token is not here at
    // all: the panel reads it from `node_token` as it shows the install command.
    if full {
        let held = (node.ipv4.as_str(), node.ipv6.as_str());
        let (pin4, pin6) = (node.ipv4_pin.as_str(), node.ipv6_pin.as_str());
        let exits = [node.ip.as_str(), node.ip_other.as_str()];
        let shown = addresses(exits, held, (pin4, pin6));
        // What each family shows with its own pin cleared and the other's kept,
        // for the edit form to offer as the fallback.
        let auto = |pins, v6: bool| {
            addresses(exits, held, pins)
                .into_iter()
                .find(|(a, _)| a.contains(':') == v6)
                .map_or("", |(a, _)| a)
        };
        view["hostname"] = json!(node.hostname);
        view["ip"] = json!(node.ip);
        view["ipv4"] = json!(node.ipv4);
        view["ipv6"] = json!(node.ipv6);
        view["ipv4_pin"] = json!(node.ipv4_pin);
        view["ipv6_pin"] = json!(node.ipv6_pin);
        view["addresses"] =
            shown.iter().map(|(address, source)| json!({"address": address, "source": source})).collect();
        view["ipv4_auto"] = json!(auto(("", pin6), false));
        view["ipv6_auto"] = json!(auto((pin4, ""), true));
        view["interval"] = json!(current.and_then(Agent::interval));
        view["country_pin"] = json!(node.country_pin);
        view["country_auto"] = json!(node.country);
        view["remark"] = json!(node.remark);
        view["notify"] = json!(node.notify);
    }
    view
}

pub(super) fn visible_nodes(app: &App, full: bool) -> Result<Vec<Value>, anyhow::Error> {
    // One traffic query and one lock for the whole list, since this is what every
    // visitor to the public page loads.
    let nodes = app.db.nodes()?;
    let traffic = app.db.all_traffic();
    let agents = app.agents.read().unwrap_or_else(|e| e.into_inner());
    let none = Traffic::default();
    let now = Local::now();
    Ok(nodes
        .iter()
        .filter(|n| full || n.public)
        .map(|n| node_view(n, agents.get(&n.id), traffic.get(&n.id).unwrap_or(&none), full, now))
        .collect())
}

pub async fn nodes(State(app): State<Shared>, headers: HeaderMap) -> Response {
    crate::blocking::response(move || {
        let full = authed(&app, &headers);
        if !full && !app.public_page() {
            return answer(StatusCode::UNAUTHORIZED, "需要登录后查看");
        }
        // The same rendered frame the browser streams receive, for the same reason:
        // otherwise every visitor would rebuild every node's row against the
        // connection the agents write through.
        //
        // Compressed likewise, once per frame: left to the compression layer, every
        // request deflates the whole list again, about 6 ms of CPU for a thousand
        // nodes, and this route answers anyone. Already encoded, the layer passes
        // it through without adding the Vary it adds to what it compresses.
        if !full && accepts_gzip(&headers) {
            return (
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::CONTENT_ENCODING, "gzip"),
                    (header::VARY, "accept-encoding"),
                ],
                match live_gzip(&app) {
                    Ok(frame) => frame,
                    Err(error) => return fail(error),
                },
            )
                .into_response();
        }
        let mut res = (
            [(header::CONTENT_TYPE, "application/json")],
            axum::body::Bytes::from(match live_snapshot(&app, full) {
                Ok(frame) => frame,
                Err(error) => return fail(error),
            }),
        )
            .into_response();
        // Uncompressed by the hub and by Cloudflare, as the admin stream is: every
        // node's address and private notes beside strings an agent reports.
        if full {
            res.headers_mut()
                .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store, no-transform"));
        }
        res
    })
    .await
}

#[derive(Deserialize)]
pub struct Window {
    #[serde(default = "default_hours")]
    pub(super) hours: i64,
    /// How many points the caller can draw. Absent means the full budget.
    pub(super) points: Option<i64>,
    /// Which half the caller will draw, `metrics` or `ping`. Each tab draws one,
    /// and the other accounted for a third to two thirds of every response. Absent
    /// means both.
    pub(super) series: Option<String>,
}

pub(super) fn default_hours() -> i64 {
    6
}

/// How many history windows are built concurrently.
///
/// `PUBLIC_HOURS` bounds what one request costs; this bounds how many may run,
/// closing the same gap `main::RELAY_GATE` and `auth::PASSWORD_GATE` close on
/// the other two paths an anonymous caller can make expensive. This is the most
/// expensive of the three: every request holds the single connection the agents
/// report through for its entire scan, measured at 118 ms for a week of four
/// probes and growing with `retention_days`. Without a gate, 120 requests from
/// one machine took the panel's own node list from 1 ms to 2.8 s.
///
/// Four, because the requests serialise on that one connection regardless: a
/// fifth in flight buys no throughput and merely places another scan ahead of
/// the next agent report. What the number actually sets is how long that wait
/// can become -- four at roughly 120 ms is half a second -- while leaving room
/// for several people opening charts simultaneously.
///
/// Refused rather than queued, as in `auth`: a queue admits the same flood,
/// merely later.
///
/// **This gate is ineffective without the `spawn_blocking` below.** The body of
/// this handler never awaits, so a permit taken and dropped within it is held
/// only while a worker thread is actually running the handler -- at most one per
/// worker, three on this hub. Measured at eight: 120 concurrent requests, zero
/// refusals, the panel still at 14 s. This is the same constraint
/// `PASSWORD_CHECKS` is sized against, approached from the other side: not a
/// value too high for the machine, but a handler that cannot hold more permits
/// than the machine has threads. Moving the scan off the runtime is what makes
/// "in flight" meaningful, and is what every other heavy query here already
/// does.
pub(super) const HISTORY_SLOTS: usize = 4;
pub(super) static HISTORY_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(HISTORY_SLOTS);

pub async fn metrics(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(w): Query<Window>,
) -> Response {
    let state = app.clone();
    let checked = crate::blocking::run(move || {
        let full = authed(&state, &headers);
        (readable(&state, full, id), state.db.retention_days())
    })
    .await;
    let (allowed, keep) = match checked {
        Ok(value) => value,
        Err(error) => return fail(error),
    };
    if !allowed {
        return answer(StatusCode::UNAUTHORIZED, "需要登录后查看");
    }
    // After the two point lookups above, so an unauthorised caller is told so
    // rather than asked to retry later.
    let Ok(permit) = HISTORY_GATE.try_acquire() else {
        return answer(StatusCode::SERVICE_UNAVAILABLE, "查询历史的请求太多，稍后再试");
    };
    let hours = w.hours.clamp(1, keep * 24);
    let span = span(hours, w.points, Utc::now().timestamp());
    let wants = |name: &str| w.series.as_deref().is_none_or(|s| s == name);
    let (want_metrics, want_ping) = (wants("metrics"), wants("ping"));
    // Off the runtime: this reads every probe result the node has retained
    // within the window, and a worker thread blocked on that scan, or on the
    // reader while another request holds it, serves nothing else.
    //
    // It is also what makes the gate above effective: the permit is held across
    // an await, so exactly four callers are inside it at once rather than however
    // many worker threads happen to exist.
    let built = crate::blocking::run(move || {
        let _permit = permit;
        // Probe names accompany the samples they label, so the page needs no
        // second request. Names only: targets and assignments remain behind
        // `Admin`. Skipped when probes were not requested, since the resources tab
        // has nothing to label and this costs a turn at the reader.
        let probes =
            if want_ping { app.db.ping_task_names(id).unwrap_or_else(|_| json!({})) } else { json!({}) };
        let metrics = if want_metrics { app.db.metrics(id, span)? } else { vec![] };
        // `loss` is per probe across the whole window, alongside the per-bucket
        // `loss` on the rows. Both are required and neither replaces the other:
        // the row figure is what a tooltip reads, while the window figure is the
        // only one that can be accurate, since the denominators it divides by are
        // gone by the time the rows are built. Additive, so a theme unaware of it
        // continues to work.
        let (ping, loss) = if want_ping { app.db.ping_records(id, span)? } else { (vec![], json!({})) };
        // `step` is the seconds each point spans, against which a row's `minutes`
        // is read; inferred from the stamps instead, it would be missing for a
        // window holding a single point. The last point is the bucket still in
        // progress and holds only the minutes elapsed.
        anyhow::Ok(json!({
            "metrics": metrics, "ping": ping, "probes": probes, "loss": loss, "step": span.step,
        }))
    })
    .await;
    match built.map_err(|e| anyhow::anyhow!(e)).and_then(|r| r) {
        Ok(body) => Json(body).into_response(),
        Err(e) => fail(e),
    }
}

/// Guards a per-node read: the panel sees everything, while the public page sees
/// only nodes explicitly published. `full` is the caller's own `authed`, passed
/// in because the handler also needs it for the window ceiling.
pub(super) fn readable(app: &App, full: bool, id: i64) -> bool {
    full || (app.public_page() && app.db.node(id).ok().flatten().is_some_and(|n| n.public))
}

/// Per-connection read buffer for both WebSocket surfaces. The 128 KiB default
/// would be tens of megabytes across a few hundred agents, for frames a few
/// hundred bytes long.
pub const SOCKET_BUFFER: usize = 4 * 1024;

/// Largest frame either socket accepts, matching the 64 KiB cap on the HTTP
/// body. That limit is a tower layer and never applies here, where the default
/// ceiling is 64 MiB -- reachable with a node's own token, for content that is
/// stored and then served to every viewer of the public page.
pub const MAX_FRAME: usize = 64 * 1024;

/// How long one rendered snapshot is reused. Just under the push interval, so
/// every tick rebuilds once and no viewer receives a stale frame twice.
pub(super) const SNAPSHOT_TTL_MS: i64 = 1_900;

/// The payload every browser stream sends, built at most once per tick however
/// many tabs are watching: the public page is anonymous, so a per-connection
/// build would make viewer count a multiplier on database work. Two slots,
/// because the admin view carries fields the public one must never expose.
pub(super) fn live_snapshot(app: &App, full: bool) -> anyhow::Result<Utf8Bytes> {
    current(app, full, |frame| frame.json.clone())
}

/// Drops the cached frames so the next push rebuilds. Without it a node just
/// added in the panel would disappear from the list until the frame expires.
pub(super) fn invalidate_snapshot(app: &App) {
    for frame in db::lock(&app.snapshot).iter_mut() {
        frame.at = 0;
    }
}

/// What one tick of a browser stream may send: the admin frame while the session
/// that opened it remains live, the public frame while the status page remains
/// open to anonymous callers, and nothing once either ceases to hold.
///
/// Both are checked every tick rather than at the handshake alone, because a
/// socket outlives both answers. The admin frame carries every node's token in
/// the clear, so one outliving its session would distribute credentials that
/// survive revocation -- the same gap `reset_token` closes on the agent side by
/// dropping its sender. The public frame is what an operator withdraws by
/// switching the status page off, and a socket opened a minute earlier would
/// continue sending it for as long as the tab stayed open: `live_ws` refuses new
/// anonymous connections from that moment and `nodes` answers them 401, leaving
/// this the only remaining route. Whatever the handshake tested must be tested
/// here as well.
pub(super) fn stream_audience(app: &App, session: Option<&str>) -> Option<bool> {
    match session {
        Some(hash) => app.db.admin_session_valid(hash).then_some(true),
        None => app.public_page().then_some(false),
    }
}

/// Live stream for the browser. Each connection runs its own timer -- simpler to
/// reason about than a fan-out channel -- over a shared snapshot, so a timer
/// costs no more than a send.
pub async fn live_ws(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(stream): Query<Stream>,
    upgrade: WebSocketUpgrade,
) -> Response {
    crate::blocking::response(move || {
        // The digest rather than the result: signing out must reach a stream already
        // running, and only the row it names can report whether it has.
        // A sibling subdomain's script opens this with the cookie too, and a
        // WebSocket is not held back by CORS: it would read the admin frame. Such a
        // handshake is answered as an anonymous one.
        let session = current_session(&headers)
            .filter(|_| !from_other_origin(&headers))
            .filter(|hash| app.db.admin_session_valid(hash));
        if session.is_none() && !app.public_page() {
            return answer(StatusCode::UNAUTHORIZED, "需要登录后查看");
        }
        let Ok(slot) = stream_slot(session.as_deref()) else {
            return answer(StatusCode::SERVICE_UNAVAILABLE, "同时在线的访客太多，请稍后再试");
        };
        upgrade.read_buffer_size(SOCKET_BUFFER).max_message_size(MAX_FRAME).on_upgrade(
            move |socket| async move {
                // Held until the stream ends.
                let _slot = slot;
                stream_live(app, socket, session, stream.gzip.is_some()).await
            },
        )
    })
    .await
}

pub(super) async fn stream_live(app: Shared, mut socket: WebSocket, session: Option<String>, gzip: bool) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticker.tick() => {},
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => continue,
            },
        }
        let state = app.clone();
        let audience = session.clone();
        let frame = crate::blocking::run(move || {
            let full = stream_audience(&state, audience.as_deref())?;
            if gzip && !full {
                if let Ok(frame) = live_gzip(&state) {
                    return Some(Message::Binary(frame));
                }
            }
            stream_frame(&state, audience.as_deref()).map(Message::Text)
        })
        .await;
        let Ok(Some(frame)) = frame else { break };
        if !matches!(tokio::time::timeout(SEND_TIMEOUT, socket.send(frame)).await, Ok(Ok(()))) {
            break;
        }
    }
}

pub(super) fn stream_frame(app: &App, session: Option<&str>) -> Option<Utf8Bytes> {
    let full = stream_audience(app, session)?;
    Some(match live_snapshot(app, full) {
        Ok(frame) => frame,
        Err(error) => {
            warn!("building live snapshot failed: {error:#}");
            let cache = db::lock(&app.snapshot);
            let mut last: Value = serde_json::from_str(cache[usize::from(full)].json.as_str())
                .unwrap_or_else(|_| json!({"nodes": null, "admin": full}));
            last["stale"] = json!(true);
            last["error"] = json!(INTERNAL);
            Utf8Bytes::from(last.to_string())
        }
    })
}

/// The window a chart request is answered with: `hours` back from `now`,
/// widened to the point boundary at or before that.
///
/// The width is capped by the retention window alone, the same for every
/// caller, because no width costs more than the week: up to `DETAIL_DAYS` a
/// request reads minute rows, 10,080 per series at most, and past it hourly
/// rows, 8,760 per series at a year, with the minute rows not yet folded, which
/// `Db::metrics` bounds to the newest week. The scan is what a request costs --
/// the thinning below bounds the response, not the rows read -- so a width
/// reading more than the week would need a lower ceiling for anonymous callers.
///
/// Thinning exists for what the screen cannot draw rather than as a convention:
/// where the samples fit, every one is sent. A chart of a hundred points reads
/// as a hundred samples taken, which for a probe is a claim about the network.
/// Whole minutes, matching the grid the metric rows sit on, and whole hours
/// past the week, so that no hourly row straddles two points.
///
/// `points` is what the caller reports it can draw, and can only lower the
/// budget: `SAMPLES` is the hub's ceiling rather than the caller's, set at a day
/// of minutes so the day's probe window returns intact.
// ponytail: the budget is per series, so a response is SAMPLES × (1 + probes) --
// bounded by how many probes the admin created, not by the caller. Four probes
// at a day is ~90 kB gzipped; if that list ever grows long, scale SAMPLES by
// the probe count.
pub(super) fn span(hours: i64, points: Option<i64>, now: i64) -> db::Span {
    const SAMPLES: i64 = 1_440;
    let budget = points.unwrap_or(SAMPLES).clamp(60, SAMPLES);
    let hourly = hours > db::DETAIL_DAYS * 24;
    let unit = if hourly { 3_600 } else { 60 };
    // Rounded up, or the budget would not be one: a window that does not divide
    // evenly would keep the finer step and exceed it. `i64::div_ceil` is still
    // unstable, and both operands are positive here.
    let step = unit * ((hours * 3_600 / unit + budget - 1) / budget).max(1);
    // Rows are bucketed by `ts / step` from the epoch, and a window opening
    // partway through a bucket would leave its first point short of the
    // `step / 60` minutes a whole one holds.
    db::Span { since: (now - hours * 3_600).div_euclid(step) * step, step, hourly }
}

/// One audience's node list, as every browser stream and `/api/nodes` send it.
#[derive(Default)]
pub struct Frame {
    /// The millisecond it was built.
    pub(super) at: i64,
    pub(super) json: Utf8Bytes,
    /// `json` gzipped, made by the first stream or `/api/nodes` request that asks for it.
    gzip: Option<axum::body::Bytes>,
}

/// Runs `read` on one audience's frame, rebuilt first once it has expired. A
/// frame is built at most once per tick however many tabs are watching: the
/// public page is anonymous, so a per-connection build would make viewer count
/// a multiplier on database work. Two slots, because the admin view carries
/// fields the public one must never expose.
fn current<T>(app: &App, full: bool, read: impl FnOnce(&mut Frame) -> T) -> anyhow::Result<T> {
    let now = Utc::now().timestamp_millis();
    // Held while the frame is built, so that tabs ticking together build it
    // once; the rest wait for it off the runtime, as for the database.
    let mut cache = db::lock(&app.snapshot);
    let frame = &mut cache[usize::from(full)];
    // A cached frame's age must be non-negative. A wall clock can step backwards
    // -- NTP correcting a fresh boot -- and against a bare upper bound the
    // resulting negative reads as young, pinning the panel to a stale frame until
    // real time catches up.
    if !(0..SNAPSHOT_TTL_MS).contains(&now.saturating_sub(frame.at)) {
        let nodes = visible_nodes(app, full)?;
        // `admin` is included so the panel's first fetch and its stream share one
        // cached frame.
        let json = json!({"nodes": nodes, "admin": full}).to_string().into();
        *frame = Frame { at: now, json, gzip: None };
    }
    Ok(read(frame))
}

/// Whether `Accept-Encoding` lists gzip with a nonzero weight. A wildcard is
/// not read as gzip: browsers name gzip itself, and any other request is still
/// compressed by the compression layer.
pub(super) fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts.next().unwrap_or_default();
            let weight = parts
                .find_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                .map_or(1.0, |q| q.parse::<f32>().unwrap_or(0.0));
            (name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip")) && weight > 0.0
        })
}

/// The public frame gzipped, for streams opened with `?gzip` and for
/// `/api/nodes`. A hundred reporting nodes are about 110 KB of JSON every two
/// seconds, 15 KB compressed, for each page left open. Compressed once per
/// frame, about a millisecond at that size, however many streams take it.
pub(super) fn live_gzip(app: &App) -> anyhow::Result<axum::body::Bytes> {
    current(app, false, |frame| {
        frame
            .gzip
            .get_or_insert_with(|| {
                let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                // Into a Vec, which cannot fail.
                let _ = std::io::Write::write_all(&mut packed, frame.json.as_bytes());
                packed.finish().unwrap_or_default().into()
            })
            .clone()
    })
}

#[derive(Deserialize)]
pub struct Stream {
    /// Present for each public frame gzipped, in a binary message; the admin
    /// frame stays text. Without it frames are text, which every theme written
    /// before it reads.
    gzip: Option<String>,
}

/// How many anonymous streams may be open at once. The fourth bound of the
/// anonymous paths, beside `HISTORY_GATE`, `main::RELAY_GATE` and
/// `auth::PASSWORD_GATE`: a stream holds a descriptor and a task for as long as
/// its page stays open, and at a hundred nodes sends about 7 KB/s of compressed
/// frames, 55 KB/s to a page that does not ask for gzip. A thousand pages are
/// 7 to 55 MB/s outbound. Beyond it a page falls back to polling `/api/nodes`,
/// whose rendered frame every viewer shares.
pub(super) const PUBLIC_STREAMS: usize = 1000;

static PUBLIC_STREAM_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(PUBLIC_STREAMS);

/// A send that blocks this long has no reader left: the pages treat 10 s
/// without a frame as a lost stream and open another. A peer that vanished
/// without a FIN would otherwise keep its slot until TCP gives up, about
/// 15 minutes.
const SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The slot a new stream holds until it ends. A session is the operator's own
/// and is not counted, so whoever holds one can always open the panel.
pub(super) fn stream_slot(
    session: Option<&str>,
) -> Result<Option<tokio::sync::SemaphorePermit<'static>>, ()> {
    if session.is_some() {
        return Ok(None);
    }
    PUBLIC_STREAM_GATE.try_acquire().map(Some).map_err(|_| ())
}
