use super::*;

/// Restricts the database to its owner.
///
/// It is the credential store: node tokens in the clear, the GitHub client
/// secret, the password hash. SQLite creates it under the umask, which at a
/// default 022 is world-readable, and the WAL and shm files hold the same rows.
///
/// Best effort: a filesystem without Unix modes still works.
pub(super) fn restrict(path: &str) {
    for file in [path.to_owned(), format!("{path}-wal"), format!("{path}-shm")] {
        own_only(&file);
    }
}

/// One file, owner-only. Also applied to the backup copy `VACUUM INTO` writes,
/// which is the entire credential store in one portable file, created under the
/// umask like any other.
pub(super) fn own_only(file: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
}

/// The `main` database's path as SQLite reports it, empty for `:memory:`.
/// Queried rather than cached so there is a single answer to which file is
/// open.
pub(super) fn main_file(conn: &Connection) -> String {
    conn.query_row("PRAGMA database_list", [], |r| r.get(2)).unwrap_or_default()
}

pub(super) fn bytes_of(file: &str) -> i64 {
    std::fs::metadata(file).map(|m| m.len() as i64).unwrap_or(0)
}

/// Bytes the database occupies. The WAL is included: committed rows remain there
/// until a checkpoint folds them into the main file, so the two together are what
/// an operator sees on disk.
pub(super) fn on_disk(file: &str) -> i64 {
    bytes_of(file) + bytes_of(&format!("{file}-wal"))
}

/// The rows behind the latency chart: one node's probe results over a window,
/// bucketed and in time order. Everything the chart draws is folded out of them
/// in [`close_bucket`].
///
/// The key is `(node_id, ts, task_id)`, so this is a seek and the rows emerge
/// sorted without a sorter, which is what allows the fold to hold one bucket at
/// a time. Asking SQLite for the summary instead cost three sorts of the whole
/// window -- two window passes and a GROUP BY -- against this single scan: on a
/// week of four probes, 284 ms against 54 ms, all of it holding the connection
/// the agents write through.
///
/// A constant because the query plan is asserted against it in
/// `rekeying_ping_record_keeps_the_rows_and_lets_the_chart_query_seek`.
pub(super) const PING_ROWS: &str = "SELECT ts/?3, task_id, latency FROM ping_record
     WHERE node_id=?1 AND ts>=?2
           AND task_id IN (SELECT task_id FROM ping_node WHERE node_id=?1)
     ORDER BY ts";

/// Turns one finished bucket into a row per probe, stamped with the bucket's
/// start so every series lands on the same grid.
///
/// Median rather than mean: one SYN retransmit is tens of milliseconds and would
/// drag a mean, and it is the reading that is wrong rather than the link.
///
/// `latency` is null when an entire bucket timed out. `loss` is the percentage
/// that did, included only when non-zero -- a healthy day is 2,880 rows, and
/// `"loss":0` on each would add 29 kB of nothing. Rounded up, so that the absence
/// of a `loss` key means no timeouts occurred: truncating would report a bucket
/// that lost 1 of 180 as clean.
pub(super) fn close_bucket(out: &mut Vec<serde_json::Value>, open: &mut Vec<(i64, Tally)>, ts: i64) {
    for (task, mut tally) in open.drain(..) {
        let answered = tally.answered();
        let mut row = serde_json::json!({"task_id": task, "ts": ts, "latency": tally.median()});
        // Only when the bucket actually varied. At the hour and six-hour windows a
        // bucket holds one sample, and a band would be a zero-height ribbon under
        // every line.
        if let (Some(lo), Some(hi)) = (tally.lo, tally.hi) {
            if hi > lo {
                row["band"] = serde_json::json!([lo, hi]);
            }
        }
        if tally.lost > 0 {
            let total = answered + tally.lost;
            row["loss"] = ((100 * tally.lost + total - 1) / total).into();
        }
        out.push(row);
    }
}

pub(super) fn country_flag(country: &str) -> String {
    if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return String::new();
    }
    country
        .to_ascii_uppercase()
        .bytes()
        .filter_map(|byte| char::from_u32(0x1f1e6 + u32::from(byte - b'A')))
        .collect()
}

pub(crate) fn displayed_proxy_name(name: &str, include_node_name: bool, country: &str) -> String {
    if !include_node_name {
        return name.trim().to_owned();
    }
    let flag = country_flag(country);
    if flag.is_empty() {
        name.trim().to_owned()
    } else {
        format!("{}{}-{}", flag, country.to_ascii_uppercase(), name.trim())
    }
}

/// The final label depends on node metadata, so compare the rendered names
/// under the connection lock rather than putting a misleading UNIQUE index on
/// the stored base name.
pub(super) fn proxy_name_in_use(
    conn: &Connection,
    node_id: i64,
    draft: &ProxyDraft,
    except: Option<i64>,
) -> Result<bool> {
    let country: String = conn.query_row(
        "SELECT CASE WHEN country_pin='' THEN country ELSE country_pin END FROM node WHERE id=?1",
        [node_id],
        |row| row.get(0),
    )?;
    let candidate = displayed_proxy_name(&draft.name, draft.include_node_name, &country).to_lowercase();
    let mut stmt = conn.prepare(
        "SELECT p.id,p.name,p.include_node_name,
                CASE WHEN n.country_pin='' THEN n.country ELSE n.country_pin END
         FROM proxies p JOIN node n ON n.id=p.node_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, name, include_node_name, country) = row?;
        if Some(id) == except {
            continue;
        }
        if displayed_proxy_name(&name, include_node_name, &country).to_lowercase() == candidate {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn port_in_use(conn: &Connection, node_id: i64, port: i64, except: Option<i64>) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM proxies WHERE node_id=?1 AND port=?2 AND enabled=1
                       AND (?3 IS NULL OR id<>?3)) OR EXISTS(SELECT 1 FROM realm_rules WHERE node_id=?1 AND listen_port=?2)",
        params![node_id, port, except],
        |r| r.get(0),
    )?)
}

pub(super) fn parse_json(r: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<serde_json::Value> {
    let raw: String = r.get(index)?;
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
    })
}

pub(super) fn counter_delta(current: i64, last: Option<i64>) -> i64 {
    match last {
        None => 0,
        Some(previous) if current >= previous => current - previous,
        Some(_) => current,
    }
}

/// SQL fragments come only from these fixed choices, never from the request.
pub(super) fn traffic_order_by(sort: &str, descending: bool, uplink: &str, downlink: &str) -> String {
    let column = match sort {
        "uplink" => uplink.to_owned(),
        "downlink" => downlink.to_owned(),
        _ => format!("({uplink}+{downlink})"),
    };
    let direction = if descending { "DESC" } else { "ASC" };
    format!("{column} {direction}")
}

pub(super) fn row_to_proxy(r: &rusqlite::Row<'_>) -> rusqlite::Result<Proxy> {
    Ok(Proxy {
        id: r.get(0)?,
        node_id: r.get(1)?,
        name: r.get(2)?,
        include_node_name: r.get(3)?,
        protocol: r.get(4)?,
        address_type: r.get(5)?,
        address: r.get(6)?,
        port: r.get(7)?,
        enabled: r.get(8)?,
        flow: r.get(9)?,
        config: parse_json(r, 10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        sort: r.get(13)?,
    })
}

pub(super) fn row_to_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        uuid: r.get(2)?,
        enabled: r.get(3)?,
        expires_at: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        proxy_count: r.get(7)?,
        traffic_limit: r.get(8)?,
        device_limit: r.get(9)?,
        traffic_reset_day: r.get(10)?,
        auto_authorize_new_proxies: r.get(11)?,
    })
}

pub(super) fn row_to_node(r: &rusqlite::Row<'_>) -> Node {
    let s = |i: &str| r.get::<_, String>(i).unwrap_or_default();
    let n = |i: &str| r.get::<_, i64>(i).unwrap_or(0);
    Node {
        id: n("id"),
        name: s("name"),
        public: r.get::<_, bool>("public").unwrap_or(true),
        sort: n("sort"),
        price: r.get::<_, f64>("price").unwrap_or(0.0),
        currency: s("currency"),
        billing_cycle: s("billing_cycle"),
        expires_at: r.get::<_, Option<String>>("expires_at").unwrap_or(None),
        remark: s("remark"),
        public_remark: s("public_remark"),
        traffic_limit: n("traffic_limit"),
        traffic_mode: s("traffic_mode"),
        traffic_reset_day: n("traffic_reset_day") as u32,
        hostname: s("hostname"),
        os: s("os"),
        kernel: s("kernel"),
        arch: s("arch"),
        virt: s("virt"),
        cpu_name: s("cpu_name"),
        cpu_cores: n("cpu_cores"),
        mem_total: n("mem_total"),
        swap_total: n("swap_total"),
        disk_total: n("disk_total"),
        agent_version: s("agent_version"),
        ip: s("ip"),
        ip_other: s("ip_other"),
        ipv4: s("ipv4"),
        ipv6: s("ipv6"),
        country: s("country"),
        country_pin: s("country_pin"),
        group: s("group_name"),
        ipv4_pin: s("ipv4_pin"),
        ipv6_pin: s("ipv6_pin"),
        last_seen: n("last_seen"),
        notify: n("notify") != 0,
        down_since: n("down_since"),
        token: s("token"),
    }
}

pub(super) fn user_reset_period(now: i64, day: u32) -> Result<String> {
    if day == 0 {
        return Ok(String::new());
    }
    let today = DateTime::from_timestamp(now, 0)
        .context("invalid reset timestamp")?
        .with_timezone(&Local)
        .date_naive();
    Ok(period_start(today, day).to_string())
}

// Called inside the writer transaction so ingestion and maintenance cannot
// clear the same period twice. Counter baselines intentionally survive resets.
pub(super) fn reset_due_user_traffic(conn: &Connection, now: i64) -> Result<()> {
    let users = {
        let mut stmt = conn.prepare(
            "SELECT id,traffic_reset_day,traffic_reset_period FROM users WHERE traffic_reset_day>0",
        )?;
        let rows =
            stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u32>(1)?, r.get::<_, String>(2)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, day, previous) in users {
        let period = user_reset_period(now, day)?;
        if period <= previous {
            continue;
        }
        if !previous.is_empty() {
            conn.execute("UPDATE proxy_user_traffic SET uplink_bytes=0,downlink_bytes=0,reset_at=?2,updated_at=?2 WHERE user_id=?1", params![id,now])?;
        }
        conn.execute("UPDATE users SET traffic_reset_period=?2 WHERE id=?1", params![id, period])?;
    }
    Ok(())
}

/// Start of the billing period containing `today`, given a reset day of month.
/// A reset day past the end of a short month lands on that month's last day.
pub fn period_start(today: NaiveDate, reset_day: u32) -> NaiveDate {
    let day = reset_day.clamp(1, 31);
    let clamped = |y: i32, m: u32| {
        let last =
            NaiveDate::from_ymd_opt(if m == 12 { y + 1 } else { y }, if m == 12 { 1 } else { m + 1 }, 1)
                .unwrap()
                .pred_opt()
                .unwrap()
                .day();
        NaiveDate::from_ymd_opt(y, m, day.min(last)).unwrap()
    };
    let this = clamped(today.year(), today.month());
    if today >= this {
        this
    } else if today.month() == 1 {
        clamped(today.year() - 1, 12)
    } else {
        clamped(today.year(), today.month() - 1)
    }
}

/// Opens a read-only connection to the database: [`Db`]'s reader, and the one
/// [`Db::backup_into`] exports through. Read-only from the open, so no statement
/// reaching it can write. It inherits none of the PRAGMAs in `SCHEMA`, so the
/// busy timeout is set again; without it, a checkpoint racing a read would
/// return SQLITE_BUSY immediately.
pub(super) fn read_only(file: &str) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// [`PING_ROWS`] for the hourly tier: the folded hours before the watermark
/// `?4`, in time order off the key.
pub(super) const PING_HOURS: &str = "SELECT ts/?3, task_id, answered, lost, latency, lo, hi FROM ping_hour
     WHERE node_id=?1 AND ts>=?2 AND ts<?4
           AND task_id IN (SELECT task_id FROM ping_node WHERE node_id=?1)
     ORDER BY ts";

/// Minute rows are kept this many days at most, and windows up to this wide are
/// drawn from them. It is the widest window where they change what is drawn: at
/// the 1,440-point budget a week is 7-minute points, where hourly rows would
/// give 168. A month is half-hour points from minute rows and hourly from the
/// tier, which a chart the width of a screen cannot tell apart.
pub const DETAIL_DAYS: i64 = 7;

/// The longest history a hub keeps. A year spans the longest billing cycle the
/// panel records, so a machine paid annually can be judged over a whole term.
/// At 100 nodes with four probes the hourly tier holds about 145 MiB for it,
/// where minute rows would hold 6.6 GiB.
pub const MAX_RETENTION_DAYS: i64 = 365;

/// History kept where the panel has never saved a value: a month. Its hourly
/// tier adds about 0.12 MiB per node with four probes to the week of minute rows
/// every setting keeps.
pub(super) const DEFAULT_RETENTION_DAYS: i64 = 30;

/// How long after an hour ends its rows may still arrive. Metric rows are
/// written as they are stamped, but probe results wait in the session until a
/// frame from a later minute arrives, and an agent may send one as seldom as
/// once an hour: its `--interval` and a probe's interval are both capped at
/// 3600 seconds.
pub(super) const LATE: i64 = 3_600 + 60;

/// Setting holding the start of the first hour not yet folded into the hourly
/// tier. Internal: the settings routes neither return nor accept it.
pub(super) const ROLLED: &str = "history_rolled";

/// One chart window as the database answers it.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    /// Start of the window, in unix seconds.
    pub since: i64,
    /// Seconds each returned point covers: whole minutes, or whole hours when
    /// `hourly`, so that no hourly row straddles two points.
    pub step: i64,
    /// Whether the window reaches past [`DETAIL_DAYS`] and is read from the
    /// hourly tier.
    pub hourly: bool,
}

/// Lets a caller waiting on the connection take it between the steps of a long
/// maintenance run. Dropping the guard alone does not: `std::sync::Mutex` is
/// not fair, and the thread releasing it takes it back before a woken waiter
/// runs. Without the pause, on the upgrade of 90 days of 100 nodes, a request
/// would wait up to 12 s behind a prune releasing the lock between nodes.
pub(super) fn let_waiters_in() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}

/// Set once the hub begins to stop, and checked between the steps of
/// `roll_up` and `prune`. The runtime waits on blocking work before the process
/// exits, so the catch-up after an upgrade, minutes long, would otherwise hold
/// the exit past systemd's 90-second stop timeout and end in SIGKILL. Each step
/// commits on its own, and the next pass resumes where this one stopped.
pub(super) static HALTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn halt() {
    HALTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn halted() -> bool {
    HALTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The first hour not yet folded into the hourly tier, `None` before the first
/// rollup, while every row is still a minute row.
pub(super) fn rolled(conn: &Connection) -> Result<Option<i64>> {
    let value: Option<String> =
        conn.query_row("SELECT value FROM setting WHERE key=?1", [ROLLED], |r| r.get(0)).optional()?;
    Ok(value.and_then(|v| v.parse().ok()))
}

/// The earliest `ts` across `tables`, sought node by node: every key here
/// begins with `node_id`, and `MIN(ts)` over a whole table scans it, 15.9 s at
/// 90 days of 100 nodes against 1.8 ms this way. `tables` are names from
/// [`TABLES`].
pub(super) fn oldest(conn: &Connection, tables: &[&str]) -> Result<Option<i64>> {
    let per_node: Vec<String> = tables
        .iter()
        .map(|t| format!("SELECT (SELECT MIN(ts) FROM {t} WHERE node_id=n.id) AS ts FROM node n"))
        .collect();
    let sql = format!("SELECT MIN(ts) FROM ({})", per_node.join(" UNION ALL "));
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

/// Probe results as the latency fold receives them. A stored result is a
/// single answer or a single loss; an hourly row is that hour's answers,
/// represented by their median, and its losses.
pub(super) struct Sample {
    pub(super) answered: i64,
    pub(super) lost: i64,
    pub(super) median: Option<i64>,
    pub(super) lo: Option<i64>,
    pub(super) hi: Option<i64>,
}

/// One probe's samples within one bucket.
#[derive(Default)]
pub(super) struct Tally {
    /// Each sample's median and the answers it stands for.
    pub(super) medians: Vec<(i64, i64)>,
    pub(super) lost: i64,
    pub(super) lo: Option<i64>,
    pub(super) hi: Option<i64>,
}

/// Locks `mutex`, and when it is taken, waits with this thread's share of the
/// runtime handed to another.
///
/// A vacuum or a restore holds the writer for as long as the disk takes to
/// rewrite the file: minutes, on a slow one. Requests, browser streams, agent
/// handshakes and the hourly housekeeping reach it from async tasks, and each
/// waiting in place would take a worker thread with it: on one core with a
/// browser stream open, a 13 s vacuum would hold every other request for 12.5 s.
///
/// ponytail: each waiter holds a blocking thread -- during a vacuum or a
/// restore, every open browser stream and connected agent, plus each request
/// arriving meanwhile. Past tokio's default of 512 the next waiter stalls a
/// worker again; refuse waiters past a limit, as the history charts do, if
/// hubs reach that.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(e)) => e.into_inner(),
        Err(TryLockError::WouldBlock) => {
            let wait = || mutex.lock().unwrap_or_else(|e| e.into_inner());
            // `block_in_place` panics on a current-thread runtime, which is
            // what `#[tokio::test]` runs. Off the runtime's workers -- in
            // `spawn_blocking`, or already in `block_in_place` -- it calls the
            // closure as it is.
            match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
                Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(wait),
                _ => wait(),
            }
        }
    }
}

/// The id for a new row of `table`: one past every id it has issued, deleted
/// rows included. SQLite's default reissues the highest id once its row is
/// deleted, and state the hub keeps by id -- the offline sweep's absence and
/// return times, an editor opened before the delete -- would pass to the new
/// row. The high mark lives in `setting`, so a backup carries it; without one,
/// as after an upgrade, the table's own highest id stands in.
pub(super) fn next_id(tx: &rusqlite::Transaction<'_>, table: &str) -> Result<i64> {
    let id = issued(tx, table)? + 1;
    put_setting(tx, &format!("{table}_id_issued"), &id.to_string())?;
    Ok(id)
}

/// The highest id `table` has issued, by its high mark or its rows.
pub(super) fn issued(conn: &Connection, table: &str) -> Result<i64> {
    Ok(conn.query_row(
        &format!(
            "SELECT MAX(COALESCE((SELECT CAST(value AS INTEGER) FROM setting WHERE key = ?1), 0),
                        COALESCE((SELECT MAX(id) FROM {table}), 0))"
        ),
        [format!("{table}_id_issued")],
        |r| r.get(0),
    )?)
}

/// The tables whose ids [`next_id`] issues.
pub(super) const ID_TABLES: [&str; 2] = ["node", "ping_task"];

pub(super) fn put_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

impl Span {
    /// A window read from minute rows alone.
    pub fn minutes(since: i64, step: i64) -> Self {
        Span { since, step, hourly: false }
    }
}

impl Sample {
    /// One stored result. A timeout is stored as -1: counted as lost, and kept
    /// out of the median.
    pub(super) fn result(latency: i64) -> Self {
        let answer = (latency >= 0).then_some(latency);
        Sample {
            answered: i64::from(answer.is_some()),
            lost: i64::from(answer.is_none()),
            median: answer,
            lo: answer,
            hi: answer,
        }
    }
}

impl Tally {
    pub(super) fn add(&mut self, s: Sample) {
        if let Some(median) = s.median {
            self.medians.push((median, s.answered));
        }
        self.lost += s.lost;
        self.lo = self.lo.into_iter().chain(s.lo).min();
        self.hi = self.hi.into_iter().chain(s.hi).max();
    }

    pub(super) fn answered(&self) -> i64 {
        self.medians.iter().map(|m| m.1).sum()
    }

    /// The median of the answers, each sample's median counted once per answer
    /// it stands for. Over single results that is the ordinary median, the
    /// middle two averaged. Over hourly rows it is exact while a bucket is one
    /// hour and an approximation once it spans several, since the hours'
    /// medians stand in for the answers themselves.
    pub(super) fn median(&mut self) -> Option<i64> {
        self.medians.sort_unstable();
        let total = self.answered();
        let at = |rank: i64| {
            let mut seen = 0;
            self.medians.iter().find(|m| {
                seen += m.1;
                seen >= rank
            })
        };
        // The 1-based ranks of the middle answer, or of the middle two.
        Some((at((total + 1) / 2)?.0 + at(total / 2 + 1)?.0) / 2)
    }
}
