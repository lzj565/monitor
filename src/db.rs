//! SQLite storage. A single writer connection behind a mutex: at a handful of
//! nodes reporting every few seconds, every statement here is sub-millisecond.
// ponytail: single global connection; move to a read pool if the dashboard ever
// blocks behind ingest.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tracing::info;

pub struct Db(Mutex<Connection>);

const AUTO_PROXY_PORT_MIN: u16 = 24_000;
const AUTO_PROXY_PORT_MAX: u16 = 29_999;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
-- 8 MiB of page cache. The whole working set of a few hundred nodes fits, so
-- the read paths stop going back to the filesystem.
PRAGMA cache_size = -8192;
-- Without these the WAL grows to whatever the busiest minute needed and never
-- gives the space back: a hub is a long-running process on a small VPS.
PRAGMA wal_autocheckpoint = 256;
PRAGMA journal_size_limit = 1048576;

CREATE TABLE IF NOT EXISTS setting (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS node (
  id            INTEGER PRIMARY KEY,
  name          TEXT    NOT NULL,
  -- The agent's credential, in the clear: the panel shows a node's install
  -- command whenever it is asked, so it has to be able to read it back.
  token         TEXT    NOT NULL UNIQUE,
  sort          INTEGER NOT NULL DEFAULT 0,
  public        INTEGER NOT NULL DEFAULT 1,
  price         REAL    NOT NULL DEFAULT 0,
  currency      TEXT    NOT NULL DEFAULT 'USD',
  billing_cycle TEXT    NOT NULL DEFAULT 'monthly',
  expires_at    TEXT,
  remark        TEXT    NOT NULL DEFAULT '',
  traffic_limit INTEGER NOT NULL DEFAULT 0,
  traffic_mode  TEXT    NOT NULL DEFAULT 'sum',
  traffic_reset_day INTEGER NOT NULL DEFAULT 1,
  hostname TEXT NOT NULL DEFAULT '', os TEXT NOT NULL DEFAULT '',
  kernel   TEXT NOT NULL DEFAULT '', arch TEXT NOT NULL DEFAULT '',
  virt     TEXT NOT NULL DEFAULT '', cpu_name TEXT NOT NULL DEFAULT '',
  cpu_cores INTEGER NOT NULL DEFAULT 0, mem_total INTEGER NOT NULL DEFAULT 0,
  swap_total INTEGER NOT NULL DEFAULT 0, disk_total INTEGER NOT NULL DEFAULT 0,
  agent_version TEXT NOT NULL DEFAULT '', ip TEXT NOT NULL DEFAULT '',
  ipv4 TEXT NOT NULL DEFAULT '', ipv6 TEXT NOT NULL DEFAULT '',
  -- ISO 3166-1 alpha-2, looked up from `country_ip` once per address. Empty
  -- until the lookup answers, and empty is what a node whose country nobody
  -- could tell stays: the public page just leaves the badge off.
  country TEXT NOT NULL DEFAULT '',
  -- The address `country` belongs to: a public interface address the agent
  -- reported, else `ip`. Empty when neither is public.
  country_ip TEXT NOT NULL DEFAULT '',
  -- The last answered pair `country_ip` / `country` before the current one;
  -- an address never answered does not displace it. A hello taken before
  -- every interface is up picks the other family, and the next one returns;
  -- the address returned to takes its answer back from here instead of
  -- waiting out the hourly lookup limit the detour spent.
  -- One pair suffices: a machine's sources are its v4, or the exit in front of
  -- it, and its v6.
  country_prev_ip TEXT NOT NULL DEFAULT '',
  country_prev TEXT NOT NULL DEFAULT '',
  -- Set in the panel. When not empty it is the country shown, in place of the
  -- looked-up one, which goes on updating underneath.
  country_pin TEXT NOT NULL DEFAULT '',
  -- Set in the panel and shown on the status page, where a theme may divide the
  -- node list by it. Empty is ungrouped. Not `group`, a reserved word.
  group_name TEXT NOT NULL DEFAULT '',
  -- Set in the panel, each replacing the address shown for its family. Empty
  -- means automatic. Panel only, like the reported addresses.
  ipv4_pin TEXT NOT NULL DEFAULT '', ipv6_pin TEXT NOT NULL DEFAULT '',
  -- Survives the disconnection it describes, unlike the in-memory live entry:
  -- an offline node's page is exactly where "since when" is worth reading.
  last_seen INTEGER NOT NULL DEFAULT 0,
  -- Opt-in, as the operator decides which machines are worth an alert.
  notify INTEGER NOT NULL DEFAULT 0,
  -- `last_seen` as of the offline alert, zero while none is outstanding. Stored
  -- rather than held in memory so that a hub restart neither repeats the alert
  -- nor loses the recovery that pairs with it.
  down_since INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

-- Monotonic byte counters that survive both agent reboots and hub restarts.
CREATE TABLE IF NOT EXISTS traffic (
  node_id  INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  boot_id  TEXT    NOT NULL DEFAULT '',
  last_rx  INTEGER NOT NULL DEFAULT 0,
  last_tx  INTEGER NOT NULL DEFAULT 0,
  total_rx INTEGER NOT NULL DEFAULT 0,
  total_tx INTEGER NOT NULL DEFAULT 0,
  month_rx INTEGER NOT NULL DEFAULT 0,
  month_tx INTEGER NOT NULL DEFAULT 0,
  month_start TEXT NOT NULL DEFAULT '',
  day_rx INTEGER NOT NULL DEFAULT 0,
  day_tx INTEGER NOT NULL DEFAULT 0,
  day_start TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS metric (
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  ts      INTEGER NOT NULL,
  cpu REAL NOT NULL,
  mem_used INTEGER NOT NULL, swap_used INTEGER NOT NULL, disk_used INTEGER NOT NULL,
  net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL,
  tcp INTEGER NOT NULL, udp INTEGER NOT NULL, procs INTEGER NOT NULL,
  PRIMARY KEY (node_id, ts)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS ping_task (
  id       INTEGER PRIMARY KEY,
  name     TEXT    NOT NULL,
  target   TEXT    NOT NULL,
  interval INTEGER NOT NULL DEFAULT 60,
  auto_join INTEGER NOT NULL DEFAULT 0,
  sort     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ping_node (
  task_id INTEGER NOT NULL REFERENCES ping_task(id) ON DELETE CASCADE,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  PRIMARY KEY (task_id, node_id)
);

-- Key order follows the only query there is: one node, one time window,
-- every probe. With task_id ahead of ts SQLite can seek to the node and no
-- further, then scans every record it ever kept -- see the migration in open().
CREATE TABLE IF NOT EXISTS ping_record (
  node_id INTEGER NOT NULL, task_id INTEGER NOT NULL,
  ts INTEGER NOT NULL, latency INTEGER NOT NULL,
  PRIMARY KEY (node_id, ts, task_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS session (
  token_hash TEXT    PRIMARY KEY,
  expires_at INTEGER NOT NULL
);

"#;

/// Schema revision this build expects, stamped into `PRAGMA user_version`.
/// Increment it and add a `migrate_to_N` when the schema changes under a
/// database already in service. Every migration must be:
///
/// - Additive: a new column carries a default, and no column an earlier build
///   reads is renamed or dropped. install-hub.sh rolls a hub that fails to start
///   back to the previous binary, which then runs on the migrated file.
/// - Safe to run twice: an earlier build stamps its own, lower version into a
///   newer file, and the next upgrade runs the migration again.
///
/// A new column goes into `SCHEMA` as well, for fresh files, but an index on it
/// cannot: `open` runs `SCHEMA` before migrating, and on an older file the
/// column is not there yet. `an_upgraded_release_matches_a_fresh_database`
/// holds every migration to these rules, starting from v1.0.0's schema.
const SCHEMA_VERSION: i64 = 13;

/// Adds a column older databases lack. A duplicate column indicates the
/// migration has already run; every other error must propagate.
fn add_column(conn: &Connection, table: &str, column: &str) -> Result<()> {
    match conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column}"), []) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("duplicate column name") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// True when `table`'s stored DDL contains `needle`, which is how a migration
/// determines the shape of the database it inherited.
fn schema_mentions(conn: &Connection, table: &str, needle: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name=?1 AND sql LIKE ?2",
        params![table, format!("%{needle}%")],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// One table's column names. `table` is always a [`TABLES`] entry rather than
/// caller-supplied, which is why it can be formatted into the pragma.
fn columns_of(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(names.collect::<Result<_, _>>()?)
}

/// Everything accumulated before a version was recorded. Runs once, on a
/// database predating the stamp.
fn migrate_to_1(conn: &Connection) -> Result<()> {
    for column in [
        "day_rx INTEGER NOT NULL DEFAULT 0",
        "day_tx INTEGER NOT NULL DEFAULT 0",
        "day_start TEXT NOT NULL DEFAULT ''",
    ] {
        add_column(conn, "traffic", column)?;
    }
    for column in [
        "ipv4 TEXT NOT NULL DEFAULT ''",
        "ipv6 TEXT NOT NULL DEFAULT ''",
        "last_seen INTEGER NOT NULL DEFAULT 0",
    ] {
        add_column(conn, "node", column)?;
    }
    // The column held a sha256 of the token and now holds the token itself.
    // Databases predating the change retain digests no agent can present, so
    // those nodes require a new token issued from the panel.
    if schema_mentions(conn, "node", "token_hash")? {
        conn.execute("ALTER TABLE node RENAME COLUMN token_hash TO token", [])?;
        info!("renamed node.token_hash to node.token; existing nodes need a fresh token");
    }
    // Reordering a key requires rebuilding the table; CREATE TABLE IF NOT EXISTS
    // leaves an existing one untouched. The old order placed task_id between the
    // node and the timestamp, so the chart query scanned a node's entire history
    // to answer for one hour of it: 42 ms against 0.8 ms at a month of
    // retention.
    if schema_mentions(conn, "ping_record", "(node_id, task_id, ts)")? {
        conn.execute_batch(
            "CREATE TABLE ping_record_rekeyed (
               node_id INTEGER NOT NULL, task_id INTEGER NOT NULL,
               ts INTEGER NOT NULL, latency INTEGER NOT NULL,
               PRIMARY KEY (node_id, ts, task_id)
             ) WITHOUT ROWID;
             INSERT INTO ping_record_rekeyed SELECT * FROM ping_record;
             DROP TABLE ping_record;
             ALTER TABLE ping_record_rekeyed RENAME TO ping_record;",
        )?;
        info!("rebuilt ping_record on a key the latency chart can seek");
    }
    Ok(())
}

/// `metric.load1` was written on every history row and read by nothing: the card
/// draws the live `load` array from the report, and no chart draws load from
/// history. Dropping it recovers 21% of what the five unread columns cost, and
/// it is the only one the hub can lose without also losing a figure the UI
/// displays.
///
/// The column is `NOT NULL` with no default, so this migration is mandatory:
/// without it every metric insert this build makes violates the constraint.
fn migrate_to_2(conn: &Connection) -> Result<()> {
    if schema_mentions(conn, "metric", "load1")? {
        conn.execute("ALTER TABLE metric DROP COLUMN load1", [])?;
        info!("dropped metric.load1; nothing read it");
    }
    Ok(())
}

fn migrate_to_3(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country TEXT NOT NULL DEFAULT ''")
}

fn migrate_to_4(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "notify INTEGER NOT NULL DEFAULT 0")?;
    add_column(conn, "node", "down_since INTEGER NOT NULL DEFAULT 0")
}

/// Every country stored until now was looked up from `ip`. Recording that
/// keeps the badge of a node whose lookup address is still `ip`, and has a node
/// whose public interface address now takes precedence looked up again at its
/// next hello. The columns set by hand start empty: automatic.
fn migrate_to_5(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country_ip TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "country_pin TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "ipv4_pin TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "ipv6_pin TEXT NOT NULL DEFAULT ''")?;
    conn.execute("UPDATE node SET country_ip = ip WHERE country != ''", [])?;
    Ok(())
}

fn migrate_to_6(conn: &Connection) -> Result<()> {
    add_column(conn, "ping_task", "auto_join INTEGER NOT NULL DEFAULT 0")
}

fn migrate_to_7(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "group_name TEXT NOT NULL DEFAULT ''")
}

fn migrate_to_8(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country_prev_ip TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "country_prev TEXT NOT NULL DEFAULT ''")
}

/// Every existing probe ties at 0, so `ORDER BY sort, id` keeps the id order an
/// upgraded database listed them in.
fn migrate_to_9(conn: &Connection) -> Result<()> {
    add_column(conn, "ping_task", "sort INTEGER NOT NULL DEFAULT 0")
}

/// Proxy 业务使用独立表，节点删除时清理其代理实例；用户暂不绑定节点。
fn migrate_to_10(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS proxy_instance (
           id INTEGER PRIMARY KEY,
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           engine TEXT NOT NULL CHECK (engine IN ('singbox', 'xray')),
           version TEXT,
           status TEXT NOT NULL DEFAULT 'unknown'
             CHECK (status IN ('unknown', 'not_installed', 'service_missing', 'stopped', 'running')),
           config_hash TEXT,
           config_version INTEGER NOT NULL DEFAULT 0 CHECK (config_version >= 0),
           config_updated_at INTEGER,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           UNIQUE (node_id, engine)
         );
         CREATE TABLE IF NOT EXISTS proxy_user (
           id INTEGER PRIMARY KEY,
           username TEXT NOT NULL UNIQUE,
           uuid TEXT NOT NULL UNIQUE,
           enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
           quota INTEGER NOT NULL DEFAULT 0 CHECK (quota >= 0),
           expire_at INTEGER,
           created_at INTEGER NOT NULL
         );",
    )?;
    Ok(())
}

/// VLESS + Reality 节点与服务器及 sing-box 实例分别建模。
fn migrate_to_11(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS proxy_node (
           id INTEGER PRIMARY KEY,
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           name TEXT NOT NULL CHECK (length(trim(name)) > 0),
           enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
           protocol TEXT NOT NULL CHECK (protocol = 'vless_reality'),
           address_mode TEXT NOT NULL CHECK (address_mode IN ('ipv4', 'ipv6', 'custom')),
           custom_address TEXT,
           listen_port INTEGER NOT NULL CHECK (listen_port BETWEEN 1 AND 65535),
           uuid TEXT NOT NULL CHECK (length(trim(uuid)) > 0),
           reality_private_key TEXT NOT NULL CHECK (length(trim(reality_private_key)) > 0),
           reality_public_key TEXT NOT NULL CHECK (length(trim(reality_public_key)) > 0),
           reality_short_id TEXT NOT NULL CHECK (length(trim(reality_short_id)) > 0),
           reality_server_name TEXT NOT NULL CHECK (length(trim(reality_server_name)) > 0),
           reality_dest TEXT NOT NULL CHECK (length(trim(reality_dest)) > 0),
           deploy_status TEXT NOT NULL DEFAULT 'not_deployed',
           last_error TEXT,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           UNIQUE (node_id, listen_port),
           CHECK (
             (address_mode = 'custom' AND custom_address IS NOT NULL AND length(trim(custom_address)) > 0)
             OR (address_mode IN ('ipv4', 'ipv6') AND (custom_address IS NULL OR length(trim(custom_address)) = 0))
           )
         );",
    )?;
    Ok(())
}

/// 导入时保留原 inbound 的监听地址，并在 apply 结果未确认时保留来源 tag 以便重试。
fn migrate_to_12(conn: &Connection) -> Result<()> {
    add_column(conn, "proxy_node", "listen_address TEXT NOT NULL DEFAULT '::'")?;
    add_column(conn, "proxy_node", "source_inbound_tag TEXT")?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS proxy_node_source_tag
           ON proxy_node(node_id, source_inbound_tag)
           WHERE source_inbound_tag IS NOT NULL AND length(trim(source_inbound_tag)) > 0;",
    )?;
    Ok(())
}

/// 为代理用户增加备注与更新时间，并建立到 ProxyNode 的授权关系。
fn migrate_to_13(conn: &Connection) -> Result<()> {
    add_column(conn, "proxy_user", "note TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "proxy_user", "updated_at INTEGER NOT NULL DEFAULT 0")?;
    conn.execute("UPDATE proxy_user SET updated_at=created_at WHERE updated_at=0", [])?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS proxy_user_node (
           user_id INTEGER NOT NULL REFERENCES proxy_user(id) ON DELETE CASCADE,
           proxy_node_id INTEGER NOT NULL REFERENCES proxy_node(id) ON DELETE CASCADE,
           PRIMARY KEY (user_id, proxy_node_id)
         );",
    )?;
    Ok(())
}

/// Brings a database already in service up to `SCHEMA_VERSION` and stamps it.
/// `from` is its current version. A fresh file runs the additive migration
/// history inside one transaction as well.
///
/// One transaction covers every step and the stamp. SQLite rolls back schema
/// changes and `user_version` alike, so a failure part-way -- a full disk, a
/// killed process -- leaves the file at the version it started from rather than
/// between two.
///
/// Restoring a backup also arrives here: the copy carries its own version and
/// requires the same migrations a restart would have run.
fn migrate(conn: &Connection, from: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    if from < 1 {
        migrate_to_1(&tx)?;
    }
    if from < 2 {
        migrate_to_2(&tx)?;
    }
    if from < 3 {
        migrate_to_3(&tx)?;
    }
    if from < 4 {
        migrate_to_4(&tx)?;
    }
    if from < 5 {
        migrate_to_5(&tx)?;
    }
    if from < 6 {
        migrate_to_6(&tx)?;
    }
    if from < 7 {
        migrate_to_7(&tx)?;
    }
    if from < 8 {
        migrate_to_8(&tx)?;
    }
    if from < 9 {
        migrate_to_9(&tx)?;
    }
    if from < 10 {
        migrate_to_10(&tx)?;
    }
    if from < 11 {
        migrate_to_11(&tx)?;
    }
    if from < 12 {
        migrate_to_12(&tx)?;
    }
    if from < 13 {
        migrate_to_13(&tx)?;
    }
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}

/// v1 到 v9 的备份中还没有代理实例和用户表。
const LEGACY_TABLES: [&str; 8] =
    ["setting", "node", "traffic", "metric", "ping_task", "ping_node", "ping_record", "session"];
/// v10 的备份中还没有代理节点表。
const V10_TABLES: [&str; 10] = [
    "setting",
    "node",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
    "proxy_instance",
    "proxy_user",
];
/// v11 的备份中 ProxyNode 尚未保存监听地址和导入来源 tag。
const V11_TABLES: [&str; 11] = [
    "setting",
    "node",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
    "proxy_instance",
    "proxy_user",
    "proxy_node",
];
/// v12 备份中还没有代理用户与 ProxyNode 的授权关系表。
const V12_TABLES: [&str; 11] = [
    "setting",
    "node",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
    "proxy_instance",
    "proxy_user",
    "proxy_node",
];
/// 执行当前版本迁移后，备份必须包含的全部表。
const TABLES: [&str; 12] = [
    "setting",
    "node",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
    "proxy_instance",
    "proxy_user",
    "proxy_node",
    "proxy_user_node",
];

/// One node's stored configuration and last known facts.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Node {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub public: bool,
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub price: f64,
    #[serde(default = "usd")]
    pub currency: String,
    #[serde(default = "monthly")]
    pub billing_cycle: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub remark: String,
    /// Monthly allowance in bytes; 0 means unmetered.
    #[serde(default)]
    pub traffic_limit: i64,
    /// How the allowance is counted: sum, max, up or down.
    #[serde(default = "sum")]
    pub traffic_mode: String,
    #[serde(default = "one")]
    pub traffic_reset_day: u32,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub virt: String,
    #[serde(default)]
    pub cpu_name: String,
    #[serde(default)]
    pub cpu_cores: i64,
    #[serde(default)]
    pub mem_total: i64,
    #[serde(default)]
    pub swap_total: i64,
    #[serde(default)]
    pub disk_total: i64,
    #[serde(default)]
    pub agent_version: String,
    #[serde(default)]
    pub ip: String,
    /// Reported by the agent from its own interfaces, unlike `ip`, which is
    /// merely the address the agent's connection originated from.
    #[serde(default)]
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
    /// ISO 3166-1 alpha-2, uppercase, or empty when unknown; see
    /// `agent_ws::country_source` for the address it is looked up from. Public:
    /// it appears on the status page beside the node's name.
    #[serde(default)]
    pub country: String,
    /// Set in the panel: two uppercase letters, or empty for the looked-up
    /// `country`. What the status page shows is this when present.
    #[serde(default)]
    pub country_pin: String,
    /// Set in the panel; empty is ungrouped. Public, like the name.
    #[serde(default)]
    pub group: String,
    /// Set in the panel, in canonical form, for what neither agent nor hub can
    /// know: the home line behind a transparent proxy, or which of several public
    /// addresses to show. Each replaces the address shown for its family; empty
    /// is automatic. Panel only, like `ip`.
    #[serde(default)]
    pub ipv4_pin: String,
    #[serde(default)]
    pub ipv6_pin: String,
    /// Unix seconds of the node's last report, written once a minute alongside
    /// the metric row. Zero for a node that has never reported.
    #[serde(default)]
    pub last_seen: i64,
    /// Whether going offline and coming back are announced. See `notify`.
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub down_since: i64,
    /// What the agent authenticates with. Readable so the panel can display an
    /// install command on demand; it never leaves the admin view.
    #[serde(default)]
    pub token: String,
}

/// 代理核心信息单独保存，避免把 sing-box 字段混入服务器节点记录。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProxyInstance {
    pub id: i64,
    pub node_id: i64,
    pub engine: String,
    pub version: Option<String>,
    pub status: String,
    pub config_hash: Option<String>,
    pub config_version: i64,
    pub config_updated_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 代理用户身份独立于 Monitor 管理员和服务器节点。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProxyUser {
    pub id: i64,
    pub name: String,
    pub uuid: String,
    pub enabled: bool,
    pub created_at: i64,
    pub note: String,
    pub updated_at: i64,
    pub proxy_node_ids: Vec<i64>,
}

/// 新建节点的默认监听地址；导入节点会保存服务器配置中的实际地址。
pub const DEFAULT_PROXY_LISTEN_ADDRESS: &str = "::";

/// VLESS + Reality 业务节点包含私钥，因此不实现 `Debug`，避免意外打印到日志。
#[derive(Serialize, Clone)]
pub struct ProxyNode {
    pub id: i64,
    pub node_id: i64,
    pub name: String,
    pub enabled: bool,
    pub protocol: String,
    pub address_mode: String,
    pub custom_address: Option<String>,
    pub listen_port: u16,
    pub uuid: String,
    #[serde(skip_serializing)]
    pub reality_private_key: String,
    pub reality_public_key: String,
    pub reality_short_id: String,
    pub reality_server_name: String,
    pub reality_dest: String,
    pub listen_address: String,
    #[serde(skip_serializing)]
    pub source_inbound_tag: Option<String>,
    pub deploy_status: String,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 创建或替换代理节点期望值的请求字段，不接收部署状态和时间戳。
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProxyNodeConfig {
    pub node_id: i64,
    pub name: String,
    pub enabled: bool,
    pub protocol: String,
    pub address_mode: String,
    #[serde(default)]
    pub custom_address: Option<String>,
    pub listen_port: u16,
    pub uuid: String,
    pub reality_private_key: String,
    pub reality_public_key: String,
    pub reality_short_id: String,
    pub reality_server_name: String,
    pub reality_dest: String,
}

/// 普通编辑只接受业务字段；所属服务器和凭据不能经此接口修改。
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ProxyNodeUpdate {
    pub name: String,
    pub enabled: bool,
    pub address_mode: String,
    #[serde(default)]
    pub custom_address: Option<String>,
    pub listen_port: u16,
    pub reality_server_name: String,
    pub reality_dest: String,
}

impl ProxyNodeUpdate {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            refuse!("请填写代理节点名称");
        }
        if !matches!(self.address_mode.as_str(), "ipv4" | "ipv6" | "custom") {
            refuse!("地址模式只支持 ipv4、ipv6 或 custom");
        }
        if self.listen_port == 0 {
            refuse!("监听端口必须在 1 到 65535 之间");
        }
        let custom_address_set =
            self.custom_address.as_deref().is_some_and(|address| !address.trim().is_empty());
        if self.address_mode == "custom" && !custom_address_set {
            refuse!("自定义地址模式必须填写 custom_address");
        }
        if self.address_mode != "custom" && custom_address_set {
            refuse!("只有 custom 地址模式可以填写 custom_address");
        }
        crate::proxy_config::validate_reality_settings(&self.reality_server_name, &self.reality_dest)
            .map_err(|error| anyhow::Error::msg(crate::Shown(error.to_string())))?;
        Ok(())
    }
}

fn validate_proxy_node(config: &ProxyNodeConfig) -> Result<()> {
    if config.node_id <= 0 {
        refuse!("请选择服务器");
    }
    if config.name.trim().is_empty() {
        refuse!("请填写代理节点名称");
    }
    if config.protocol != "vless_reality" {
        refuse!("当前仅支持 VLESS + Reality 协议");
    }
    if !matches!(config.address_mode.as_str(), "ipv4" | "ipv6" | "custom") {
        refuse!("地址模式只支持 ipv4、ipv6 或 custom");
    }
    if config.listen_port == 0 {
        refuse!("监听端口必须在 1 到 65535 之间");
    }
    for (value, label) in [
        (config.uuid.as_str(), "UUID"),
        (config.reality_private_key.as_str(), "Reality 私钥"),
        (config.reality_public_key.as_str(), "Reality 公钥"),
        (config.reality_short_id.as_str(), "Reality Short ID"),
        (config.reality_server_name.as_str(), "Reality Server Name"),
        (config.reality_dest.as_str(), "Reality Dest"),
    ] {
        if value.trim().is_empty() {
            refuse!("{label}不能为空");
        }
    }
    let custom_address_set =
        config.custom_address.as_deref().is_some_and(|address| !address.trim().is_empty());
    if config.address_mode == "custom" && !custom_address_set {
        refuse!("自定义地址模式必须填写 custom_address");
    }
    if config.address_mode != "custom" && custom_address_set {
        refuse!("只有 custom 地址模式可以填写 custom_address");
    }
    Ok(())
}

fn insert_proxy_node(conn: &Connection, config: &ProxyNodeConfig) -> Result<ProxyNode> {
    insert_proxy_node_with_source(conn, config, DEFAULT_PROXY_LISTEN_ADDRESS, None, "not_deployed")
}

fn insert_proxy_node_with_source(
    conn: &Connection,
    config: &ProxyNodeConfig,
    listen_address: &str,
    source_inbound_tag: Option<&str>,
    deploy_status: &str,
) -> Result<ProxyNode> {
    validate_proxy_node(config)?;
    validate_proxy_listen_address(listen_address)?;
    if source_inbound_tag.is_some_and(|tag| tag.trim().is_empty()) {
        refuse!("inbound 来源 tag 不能为空");
    }
    let now = Utc::now().timestamp();
    let custom_address = config.custom_address.as_deref().map(str::trim).filter(|value| !value.is_empty());
    conn.execute(
        "INSERT INTO proxy_node
           (node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
            uuid, reality_private_key, reality_public_key, reality_short_id,
            reality_server_name, reality_dest, listen_address, source_inbound_tag,
            deploy_status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        params![
            config.node_id,
            config.name.trim(),
            config.enabled,
            config.protocol,
            config.address_mode,
            custom_address,
            config.listen_port,
            config.uuid.trim(),
            config.reality_private_key.trim(),
            config.reality_public_key.trim(),
            config.reality_short_id.trim(),
            config.reality_server_name.trim(),
            config.reality_dest.trim(),
            listen_address,
            source_inbound_tag,
            deploy_status,
            now,
        ],
    )?;
    let id = conn.last_insert_rowid();
    Ok(conn.query_row(
        "SELECT id, node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
                uuid, reality_private_key, reality_public_key, reality_short_id,
                reality_server_name, reality_dest, listen_address, source_inbound_tag,
                deploy_status, last_error, created_at, updated_at
         FROM proxy_node WHERE id = ?1",
        [id],
        row_to_proxy_node,
    )?)
}

fn validate_proxy_listen_address(value: &str) -> Result<()> {
    if value.parse::<std::net::IpAddr>().is_err() {
        refuse!("sing-box inbound 监听地址必须是 IPv4 或 IPv6 地址");
    }
    Ok(())
}

fn yes() -> bool {
    true
}

/// Omitted settings stay unchanged. An explicit null clears the expiry date.
#[derive(Deserialize, Default)]
pub struct NodePatch {
    pub name: Option<String>,
    pub sort: Option<i64>,
    pub public: Option<bool>,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub billing_cycle: Option<String>,
    #[serde(default, deserialize_with = "expiry_patch")]
    pub expires_at: Option<Option<String>>,
    pub remark: Option<String>,
    pub traffic_limit: Option<i64>,
    pub traffic_mode: Option<String>,
    pub traffic_reset_day: Option<u32>,
    pub notify: Option<bool>,
    pub country_pin: Option<String>,
    pub ipv4_pin: Option<String>,
    pub ipv6_pin: Option<String>,
    pub group: Option<String>,
}

fn expiry_patch<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
pub struct TrafficPatch {
    pub total_rx: Option<i64>,
    pub total_tx: Option<i64>,
    pub month_rx: Option<i64>,
    pub month_tx: Option<i64>,
}
fn usd() -> String {
    "USD".into()
}
fn monthly() -> String {
    "monthly".into()
}
fn sum() -> String {
    "sum".into()
}
fn one() -> u32 {
    1
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct Traffic {
    pub total_rx: i64,
    pub total_tx: i64,
    pub month_rx: i64,
    pub month_tx: i64,
    pub month_start: String,
    pub day_rx: i64,
    pub day_tx: i64,
}

impl Traffic {
    /// This period's usage as the node's plan meters it. Summing both directions
    /// regardless would hold a plan billed on upload alone against the wrong
    /// figure. The traffic alert and `node_view` both read this, so the
    /// percentage an alert quotes matches the usage the pages show.
    pub fn month_used(&self, traffic_mode: &str) -> i64 {
        match traffic_mode {
            "up" => self.month_tx,
            "down" => self.month_rx,
            "max" => self.month_rx.max(self.month_tx),
            _ => self.month_rx.saturating_add(self.month_tx),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PingTask {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub interval: i64,
    #[serde(default)]
    pub nodes: Vec<i64>,
    /// Nodes created later are assigned this probe as they are added. Existing
    /// nodes follow `nodes` alone.
    #[serde(default)]
    pub auto_join: bool,
    /// The assignments the editor started from. When given on an update, only
    /// the difference between it and `nodes` is applied, so an assignment made
    /// while the editor was open -- a node joining through `auto_join` -- is
    /// not removed by a list that predates it.
    #[serde(default, skip_serializing)]
    pub base: Option<Vec<i64>>,
}

/// Restricts the database to its owner.
///
/// It is the credential store: node tokens in the clear, the GitHub client
/// secret, the password hash. SQLite creates it under the umask, which at a
/// default 022 is world-readable, and the WAL and shm files hold the same rows.
///
/// Best effort: a filesystem without Unix modes still works.
fn restrict(path: &str) {
    for file in [path.to_owned(), format!("{path}-wal"), format!("{path}-shm")] {
        own_only(&file);
    }
}

/// One file, owner-only. Also applied to the backup copy `VACUUM INTO` writes,
/// which is the entire credential store in one portable file, created under the
/// umask like any other.
fn own_only(file: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    }
}

/// The `main` database's path as SQLite reports it, empty for `:memory:`.
/// Queried rather than cached so there is a single answer to which file is
/// open.
fn main_file(conn: &Connection) -> String {
    conn.query_row("PRAGMA database_list", [], |r| r.get(2)).unwrap_or_default()
}

fn bytes_of(file: &str) -> i64 {
    std::fs::metadata(file).map(|m| m.len() as i64).unwrap_or(0)
}

/// Bytes the database occupies. The WAL is included: committed rows remain there
/// until a checkpoint folds them into the main file, so the two together are what
/// an operator sees on disk.
fn on_disk(file: &str) -> i64 {
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
const PING_ROWS: &str = "SELECT ts/?3, task_id, latency FROM ping_record
     WHERE node_id=?1 AND ts>=?2
           AND task_id IN (SELECT task_id FROM ping_node WHERE node_id=?1)
     ORDER BY ts";

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        restrict(path);

        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        migrate(&conn, version)?;
        Ok(Self(Mutex::new(conn)))
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- settings ----

    pub fn get(&self, key: &str) -> Option<String> {
        self.lookup(key).ok().flatten()
    }

    /// As [`Db::get`], with a failed read kept apart from an absent key, for a
    /// caller that would otherwise act on "nothing saved".
    pub fn lookup(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- nodes ----

    pub fn nodes(&self) -> Result<Vec<Node>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT * FROM node ORDER BY sort, id")?;
        let rows = stmt.query_map([], |r| Ok(row_to_node(r)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn node(&self, id: i64) -> Result<Option<Node>> {
        Ok(self
            .conn()
            .query_row("SELECT * FROM node WHERE id = ?1", [id], |r| Ok(row_to_node(r)))
            .optional()?)
    }

    pub fn proxy_instances(&self) -> Result<Vec<ProxyInstance>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, node_id, engine, version, status, config_hash, config_version,
                    config_updated_at, created_at, updated_at
             FROM proxy_instance ORDER BY node_id, engine",
        )?;
        let rows = stmt.query_map([], row_to_proxy_instance)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn proxy_nodes(&self) -> Result<Vec<ProxyNode>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
                    uuid, reality_private_key, reality_public_key, reality_short_id,
                    reality_server_name, reality_dest, listen_address, source_inbound_tag,
                    deploy_status, last_error, created_at, updated_at
             FROM proxy_node ORDER BY node_id, listen_port, id",
        )?;
        let rows = stmt.query_map([], row_to_proxy_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn proxy_nodes_for_node(&self, node_id: i64) -> Result<Vec<ProxyNode>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
                    uuid, reality_private_key, reality_public_key, reality_short_id,
                    reality_server_name, reality_dest, listen_address, source_inbound_tag,
                    deploy_status, last_error, created_at, updated_at
             FROM proxy_node WHERE node_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([node_id], row_to_proxy_node)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 同一条 SQL 原子更新该服务器全部代理节点的部署结果。
    pub fn set_proxy_nodes_deploy_status(
        &self,
        node_id: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<()> {
        if node_id <= 0 || !matches!(status, "not_deployed" | "deploying" | "deployed" | "failed") {
            anyhow::bail!("invalid proxy deployment status");
        }
        if last_error.is_some_and(|message| message.len() > 512) {
            anyhow::bail!("proxy deployment error summary is too long");
        }
        self.conn().execute(
            "UPDATE proxy_node SET deploy_status=?1, last_error=?2, updated_at=?3 WHERE node_id=?4",
            params![status, last_error, Utc::now().timestamp(), node_id],
        )?;
        Ok(())
    }

    pub fn set_proxy_node_deploy_status(
        &self,
        id: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<bool> {
        if id <= 0 || !matches!(status, "not_deployed" | "deploying" | "deployed" | "failed") {
            anyhow::bail!("invalid proxy deployment status");
        }
        if last_error.is_some_and(|message| message.len() > 512) {
            anyhow::bail!("proxy deployment error summary is too long");
        }
        Ok(self.conn().execute(
            "UPDATE proxy_node SET deploy_status=?1, last_error=?2, updated_at=?3 WHERE id=?4",
            params![status, last_error, Utc::now().timestamp(), id],
        )? > 0)
    }

    pub fn clear_proxy_node_source_tags(&self, node_id: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE proxy_node SET source_inbound_tag=NULL, updated_at=?1
             WHERE node_id=?2 AND source_inbound_tag IS NOT NULL",
            params![Utc::now().timestamp(), node_id],
        )?;
        Ok(())
    }

    pub fn proxy_node_by_source_tag(&self, node_id: i64, source_tag: &str) -> Result<Option<ProxyNode>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
                        uuid, reality_private_key, reality_public_key, reality_short_id,
                        reality_server_name, reality_dest, listen_address, source_inbound_tag,
                        deploy_status, last_error, created_at, updated_at
                 FROM proxy_node WHERE node_id=?1 AND source_inbound_tag=?2",
                params![node_id, source_tag],
                row_to_proxy_node,
            )
            .optional()?)
    }

    pub fn proxy_node(&self, id: i64) -> Result<Option<ProxyNode>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, node_id, name, enabled, protocol, address_mode, custom_address, listen_port,
                        uuid, reality_private_key, reality_public_key, reality_short_id,
                        reality_server_name, reality_dest, listen_address, source_inbound_tag,
                        deploy_status, last_error, created_at, updated_at
                 FROM proxy_node WHERE id = ?1",
                [id],
                row_to_proxy_node,
            )
            .optional()?)
    }

    pub fn create_proxy_node(&self, config: &ProxyNodeConfig) -> Result<ProxyNode> {
        let conn = self.conn();
        insert_proxy_node(&conn, config)
    }

    pub fn create_imported_proxy_node(
        &self,
        config: &ProxyNodeConfig,
        listen_address: &str,
        source_inbound_tag: &str,
    ) -> Result<ProxyNode> {
        let conn = self.conn();
        insert_proxy_node_with_source(&conn, config, listen_address, Some(source_inbound_tag), "deploying")
    }

    /// 在事务中分配端口并创建记录，避免并发请求选中同一端口。
    pub fn create_proxy_node_auto_port(
        &self,
        config: &ProxyNodeConfig,
        preferred_port: u16,
    ) -> Result<ProxyNode> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let used = {
            let mut stmt = tx.prepare("SELECT listen_port FROM proxy_node WHERE node_id=?1")?;
            let ports = stmt
                .query_map([config.node_id], |row| row.get::<_, u16>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            ports
        };
        let port_count = u32::from(AUTO_PROXY_PORT_MAX - AUTO_PROXY_PORT_MIN) + 1;
        let start = if (AUTO_PROXY_PORT_MIN..=AUTO_PROXY_PORT_MAX).contains(&preferred_port) {
            preferred_port
        } else {
            AUTO_PROXY_PORT_MIN
        };
        let listen_port = (0..port_count)
            .map(|offset| {
                AUTO_PROXY_PORT_MIN + ((u32::from(start - AUTO_PROXY_PORT_MIN) + offset) % port_count) as u16
            })
            .find(|port| !used.contains(port));
        let Some(listen_port) = listen_port else {
            refuse!("该服务器没有可自动分配的监听端口");
        };
        let mut allocated = config.clone();
        allocated.listen_port = listen_port;
        let created = insert_proxy_node(&tx, &allocated)?;
        tx.commit()?;
        Ok(created)
    }

    pub fn update_proxy_node(&self, id: i64, config: &ProxyNodeUpdate) -> Result<bool> {
        config.validate()?;
        let now = Utc::now().timestamp();
        let custom_address =
            config.custom_address.as_deref().map(str::trim).filter(|value| !value.is_empty());
        Ok(self.conn().execute(
            "UPDATE proxy_node SET name=?2, enabled=?3, address_mode=?4,
                    custom_address=?5, listen_port=?6, reality_server_name=?7,
                    reality_dest=?8, deploy_status='not_deployed', last_error=NULL, updated_at=?9
             WHERE id=?1",
            params![
                id,
                config.name.trim(),
                config.enabled,
                config.address_mode,
                custom_address,
                config.listen_port,
                config.reality_server_name.trim(),
                config.reality_dest.trim(),
                now,
            ],
        )? > 0)
    }

    pub fn regenerate_proxy_node_credentials(
        &self,
        id: i64,
        uuid: Option<&str>,
        reality_private_key: Option<&str>,
        reality_public_key: Option<&str>,
        reality_short_id: Option<&str>,
    ) -> Result<bool> {
        if id <= 0 {
            anyhow::bail!("invalid proxy node id");
        }
        if uuid.is_none()
            && reality_private_key.is_none()
            && reality_public_key.is_none()
            && reality_short_id.is_none()
        {
            anyhow::bail!("no proxy credential selected for regeneration");
        }
        if reality_private_key.is_some() != reality_public_key.is_some() {
            anyhow::bail!("Reality keys must be regenerated as a pair");
        }
        Ok(self.conn().execute(
            "UPDATE proxy_node SET uuid=COALESCE(?2, uuid),
                    reality_private_key=COALESCE(?3, reality_private_key),
                    reality_public_key=COALESCE(?4, reality_public_key),
                    reality_short_id=COALESCE(?5, reality_short_id),
                    deploy_status='not_deployed', last_error=NULL, updated_at=?6
             WHERE id=?1",
            params![
                id,
                uuid.map(str::trim),
                reality_private_key,
                reality_public_key,
                reality_short_id,
                Utc::now().timestamp()
            ],
        )? > 0)
    }

    pub fn delete_proxy_node(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM proxy_node WHERE id=?1", [id])? > 0)
    }

    /// 只保存白名单状态观测和配置摘要；嵌套 Option 用于区分字段未提供与显式 null。
    pub fn observe_singbox(
        &self,
        node_id: i64,
        status: Option<&str>,
        version_update: Option<Option<&str>>,
        config_hash: Option<&str>,
        config_updated_at: Option<i64>,
        now: i64,
    ) -> Result<()> {
        if let Some(status) = status {
            if !matches!(status, "unknown" | "not_installed" | "service_missing" | "stopped" | "running") {
                anyhow::bail!("invalid sing-box status");
            }
        }
        if let Some(hash) = config_hash {
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                anyhow::bail!("invalid sing-box config hash");
            }
        }
        let version_observed = version_update.is_some();
        let version = version_update.flatten();
        self.conn().execute(
            "INSERT INTO proxy_instance
               (node_id, engine, version, status, config_hash, config_version,
                config_updated_at, created_at, updated_at)
             VALUES (?1, 'singbox', ?2, COALESCE(?3, 'unknown'), ?4,
                     CASE WHEN ?4 IS NULL THEN 0 ELSE 1 END, ?5, ?6, ?6)
             ON CONFLICT(node_id, engine) DO UPDATE SET
               version = CASE WHEN ?7 THEN ?2 ELSE proxy_instance.version END,
               status = COALESCE(?3, proxy_instance.status),
               config_version = proxy_instance.config_version +
                 CASE WHEN ?4 IS NOT NULL AND
                   (proxy_instance.config_hash IS NULL OR proxy_instance.config_hash != ?4)
                   THEN 1 ELSE 0 END,
               config_hash = COALESCE(?4, proxy_instance.config_hash),
               config_updated_at = CASE WHEN ?4 IS NOT NULL THEN ?5 ELSE proxy_instance.config_updated_at END,
               updated_at = ?6",
            params![node_id, version, status, config_hash, config_updated_at, now, version_observed],
        )?;
        Ok(())
    }

    pub fn proxy_user(&self, id: i64) -> Result<Option<ProxyUser>> {
        let conn = self.conn();
        let mut user = conn
            .query_row(
                "SELECT id, username AS name, uuid, enabled, created_at, note, updated_at
                 FROM proxy_user WHERE id = ?1",
                [id],
                row_to_proxy_user,
            )
            .optional()?;
        if let Some(user) = &mut user {
            user.proxy_node_ids = proxy_user_node_ids(&conn, id)?;
        }
        Ok(user)
    }

    pub fn proxy_users(&self) -> Result<Vec<ProxyUser>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, username AS name, uuid, enabled, created_at, note, updated_at
             FROM proxy_user ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], row_to_proxy_user)?;
        let mut users = rows.collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        for user in &mut users {
            user.proxy_node_ids = proxy_user_node_ids(&conn, user.id)?;
        }
        Ok(users)
    }

    /// 创建用户及其节点授权；授权关系和用户记录必须同时提交。
    pub fn create_proxy_user_with_nodes(
        &self,
        name: &str,
        uuid: &str,
        enabled: bool,
        note: &str,
        requested_node_ids: &[i64],
    ) -> Result<(ProxyUser, Vec<i64>)> {
        validate_proxy_user_profile(name, note, uuid)?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_proxy_user_name_available(&tx, name, None)?;
        let proxy_node_ids = validated_proxy_node_ids(&tx, requested_node_ids)?;
        let now = Utc::now().timestamp();
        tx.execute(
            "INSERT INTO proxy_user
               (username, uuid, enabled, created_at, note, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?4)",
            params![name.trim(), uuid.trim(), enabled, now, note.trim()],
        )?;
        let id = tx.last_insert_rowid();
        for proxy_node_id in &proxy_node_ids {
            tx.execute(
                "INSERT INTO proxy_user_node (user_id, proxy_node_id) VALUES (?1, ?2)",
                params![id, proxy_node_id],
            )?;
        }
        let server_ids = proxy_server_ids_for_nodes(&tx, &proxy_node_ids)?;
        let mut user =
            query_proxy_user(&tx, id)?.ok_or_else(|| anyhow::anyhow!("created proxy user missing"))?;
        user.proxy_node_ids = proxy_node_ids;
        tx.commit()?;
        Ok((user, server_ids))
    }

    /// 更新 desired state 与授权，并返回旧、新授权涉及的去重服务器。
    pub fn update_proxy_user_profile(
        &self,
        id: i64,
        name: &str,
        enabled: bool,
        note: &str,
        requested_node_ids: &[i64],
    ) -> Result<Option<(ProxyUser, Vec<i64>)>> {
        validate_proxy_user_name_note(name, note)?;
        if id <= 0 {
            refuse!("代理用户 ID 无效");
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut user) = query_proxy_user(&tx, id)? else {
            return Ok(None);
        };
        ensure_proxy_user_name_available(&tx, name, Some(id))?;
        let old_proxy_node_ids = proxy_user_node_ids(&tx, id)?;
        let proxy_node_ids = validated_proxy_node_ids(&tx, requested_node_ids)?;
        let mut server_ids = proxy_server_ids_for_nodes(&tx, &old_proxy_node_ids)?;
        server_ids.extend(proxy_server_ids_for_nodes(&tx, &proxy_node_ids)?);
        server_ids.sort_unstable();
        server_ids.dedup();

        let now = Utc::now().timestamp();
        tx.execute(
            "UPDATE proxy_user SET username=?2, enabled=?3, note=?4, updated_at=?5 WHERE id=?1",
            params![id, name.trim(), enabled, note.trim(), now],
        )?;
        tx.execute("DELETE FROM proxy_user_node WHERE user_id=?1", [id])?;
        for proxy_node_id in &proxy_node_ids {
            tx.execute(
                "INSERT INTO proxy_user_node (user_id, proxy_node_id) VALUES (?1, ?2)",
                params![id, proxy_node_id],
            )?;
        }
        user.name = name.trim().to_owned();
        user.enabled = enabled;
        user.note = note.trim().to_owned();
        user.updated_at = now;
        user.proxy_node_ids = proxy_node_ids;
        tx.commit()?;
        Ok(Some((user, server_ids)))
    }

    /// 更新 UUID 后返回该用户当前授权涉及的服务器，供调用方同步 desired state。
    pub fn regenerate_proxy_user_uuid(&self, id: i64, uuid: &str) -> Result<Option<(ProxyUser, Vec<i64>)>> {
        if id <= 0 || !crate::proxy_config::is_uuid(uuid) {
            refuse!("代理用户 UUID 无效");
        }
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut user) = query_proxy_user(&tx, id)? else {
            return Ok(None);
        };
        let proxy_node_ids = proxy_user_node_ids(&tx, id)?;
        let server_ids = proxy_server_ids_for_nodes(&tx, &proxy_node_ids)?;
        let now = Utc::now().timestamp();
        tx.execute("UPDATE proxy_user SET uuid=?2, updated_at=?3 WHERE id=?1", params![id, uuid, now])?;
        user.uuid = uuid.to_owned();
        user.updated_at = now;
        user.proxy_node_ids = proxy_node_ids;
        tx.commit()?;
        Ok(Some((user, server_ids)))
    }

    pub fn proxy_user_server_ids(&self, id: i64) -> Result<Vec<i64>> {
        let conn = self.conn();
        let proxy_node_ids = proxy_user_node_ids(&conn, id)?;
        let mut server_ids = proxy_server_ids_for_nodes(&conn, &proxy_node_ids)?;
        // 授权被移除后，失败服务器不再能从当前关系表反查；ProxyNode 的部署状态是服务器级快照，重试时一并纳入。
        let mut stmt = conn.prepare(
            "SELECT DISTINCT node_id FROM proxy_node
             WHERE deploy_status IN ('failed', 'deploying') ORDER BY node_id",
        )?;
        let pending_ids = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        server_ids.extend(pending_ids.collect::<Result<Vec<_>, _>>()?);
        server_ids.sort_unstable();
        server_ids.dedup();
        Ok(server_ids)
    }

    /// 返回这个服务器上至少授权了一个 ProxyNode 的用户，包含停用用户供生成器明确过滤。
    pub fn proxy_users_for_node(&self, node_id: i64) -> Result<Vec<ProxyUser>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT pu.id, pu.username AS name, pu.uuid, pu.enabled,
                    pu.created_at, pu.note, pu.updated_at, pun.proxy_node_id
             FROM proxy_user pu
             JOIN proxy_user_node pun ON pun.user_id=pu.id
             JOIN proxy_node pn ON pn.id=pun.proxy_node_id
             WHERE pn.node_id=?1 ORDER BY pu.id, pun.proxy_node_id",
        )?;
        let rows = stmt.query_map([node_id], |row| {
            Ok((
                ProxyUser {
                    id: row.get("id")?,
                    name: row.get("name")?,
                    uuid: row.get("uuid")?,
                    enabled: row.get("enabled")?,
                    created_at: row.get("created_at")?,
                    note: row.get("note")?,
                    updated_at: row.get("updated_at")?,
                    proxy_node_ids: Vec::new(),
                },
                row.get::<_, i64>("proxy_node_id")?,
            ))
        })?;
        let mut users = BTreeMap::new();
        for row in rows {
            let (user, proxy_node_id) = row?;
            users.entry(user.id).or_insert(user).proxy_node_ids.push(proxy_node_id);
        }
        Ok(users.into_values().collect())
    }

    pub fn delete_proxy_user(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM proxy_user WHERE id=?1", [id])? > 0)
    }

    /// Creates a node and returns its id.
    ///
    /// One transaction: `accumulate` reads the `traffic` row on every report, so
    /// a node lacking one cannot report. The node also takes every `auto_join`
    /// probe here, at most [`Self::MAX_PROBES_PER_NODE`] rows, a limit
    /// `save_ping_task` enforces.
    pub fn create_node(&self, n: &Node, token: &str) -> Result<i64> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            // A new node belongs at the end. The caller sends sort 0, which would
            // tie with whatever the last reorder placed first.
            "INSERT INTO node (name, token, sort, public, price, currency, billing_cycle,
                               expires_at, remark, traffic_limit, traffic_mode, traffic_reset_day, created_at,
                               group_name)
             VALUES (?1,?2,(SELECT COALESCE(MAX(sort),-1)+1 FROM node),?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                n.name,
                token,
                n.public,
                n.price,
                n.currency,
                n.billing_cycle,
                n.expires_at,
                n.remark,
                n.traffic_limit,
                n.traffic_mode,
                n.traffic_reset_day,
                Utc::now().timestamp(),
                n.group
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO traffic (node_id) VALUES (?1)", [id])?;
        tx.execute(
            "INSERT INTO ping_node (task_id, node_id) SELECT id, ?1 FROM ping_task WHERE auto_join",
            [id],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// How many nodes were created at or after `ts`. Bounds what one registration
    /// window can add; see `api::REGISTER_LIMIT`.
    pub fn nodes_created_since(&self, ts: i64) -> Result<i64> {
        Ok(self.conn().query_row("SELECT COUNT(*) FROM node WHERE created_at >= ?1", [ts], |r| r.get(0))?)
    }

    /// Records when the node last reported, with the capacities that report
    /// carried. Written with each metric row and once more as the session ends,
    /// so an offline node shows the disk it last had rather than the one it
    /// connected with. A capacity absent from `metrics` keeps its stored value.
    pub fn touch_seen(&self, id: i64, ts: i64, metrics: &serde_json::Value) -> Result<()> {
        let n = |k: &str| metrics.get(k).and_then(serde_json::Value::as_i64);
        self.conn()
            .prepare_cached(
                "UPDATE node SET last_seen=?2, mem_total=COALESCE(?3,mem_total),
                                 swap_total=COALESCE(?4,swap_total), disk_total=COALESCE(?5,disk_total)
                 WHERE id=?1",
            )?
            .execute(params![id, ts, n("mem_total"), n("swap_total"), n("disk_total")])?;
        Ok(())
    }

    /// False when no node has this id.
    pub fn update_node(&self, id: i64, n: &NodePatch) -> Result<bool> {
        self.update_nodes(&[id], n)
    }

    /// Applies one patch to every node in `ids` in a single transaction. False,
    /// with nothing written, when any of them no longer exists: a batch applied
    /// to part of what was selected would leave the panel to work out which part.
    pub fn update_nodes(&self, ids: &[i64], n: &NodePatch) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut update = tx.prepare(
                "UPDATE node SET name=COALESCE(?2,name), sort=COALESCE(?3,sort), public=COALESCE(?4,public),
                                 price=COALESCE(?5,price), currency=COALESCE(?6,currency),
                                 billing_cycle=COALESCE(?7,billing_cycle),
                                 expires_at=CASE WHEN ?8 THEN ?9 ELSE expires_at END,
                                 remark=COALESCE(?10,remark), traffic_limit=COALESCE(?11,traffic_limit),
                                 traffic_mode=COALESCE(?12,traffic_mode),
                                 traffic_reset_day=COALESCE(?13,traffic_reset_day),
                                 notify=COALESCE(?14,notify), country_pin=COALESCE(?15,country_pin),
                                 ipv4_pin=COALESCE(?16,ipv4_pin), ipv6_pin=COALESCE(?17,ipv6_pin),
                                 group_name=COALESCE(?18,group_name)
                 WHERE id=?1",
            )?;
            for id in ids {
                let found = update.execute(params![
                    id,
                    n.name,
                    n.sort,
                    n.public,
                    n.price,
                    n.currency,
                    n.billing_cycle,
                    n.expires_at.is_some(),
                    n.expires_at.as_ref().and_then(|v| v.as_deref()),
                    n.remark,
                    n.traffic_limit,
                    n.traffic_mode,
                    n.traffic_reset_day,
                    n.notify,
                    n.country_pin,
                    n.ipv4_pin,
                    n.ipv6_pin,
                    n.group
                ])?;
                // Dropping the transaction uncommitted rolls back the nodes
                // already updated.
                if found == 0 {
                    return Ok(false);
                }
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn set_expiry(&self, id: i64, date: &str) -> Result<()> {
        self.conn().execute("UPDATE node SET expires_at=?2 WHERE id=?1", params![id, date])?;
        Ok(())
    }

    pub fn set_down_since(&self, id: i64, ts: i64) -> Result<()> {
        self.conn().execute("UPDATE node SET down_since=?2 WHERE id=?1", params![id, ts])?;
        Ok(())
    }

    pub fn reorder_nodes(&self, ids: &[i64]) -> Result<()> {
        self.reorder("node", ids)
    }

    pub fn reorder_ping_tasks(&self, ids: &[i64]) -> Result<()> {
        self.reorder("ping_task", ids)
    }

    /// Renumbers `sort` from a list that must name every row exactly once, so a
    /// tab that missed an insert or a delete cannot renumber around it.
    fn reorder(&self, table: &str, ids: &[i64]) -> Result<()> {
        let unique: HashSet<_> = ids.iter().collect();
        if unique.len() != ids.len() {
            refuse!("排序里有重复的条目");
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let count: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
        if count as usize != ids.len() {
            refuse!("列表已在别处改动，刷新后再排序");
        }
        let sql = format!("UPDATE {table} SET sort=?2 WHERE id=?1");
        for (sort, id) in ids.iter().enumerate() {
            if tx.execute(&sql, params![id, sort as i64])? != 1 {
                refuse!("列表已在别处改动，刷新后再排序");
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// False when no node has this id.
    pub fn delete_node(&self, id: i64) -> Result<bool> {
        let conn = self.conn();
        // `ping_record` carries no foreign key -- it is WITHOUT ROWID and keyed
        // for the chart query -- so it is cleared explicitly. SQLite reassigns a
        // deleted node's id to the next node created, which would otherwise
        // inherit the removed machine's latency chart.
        conn.execute("DELETE FROM ping_record WHERE node_id = ?1", [id])?;
        Ok(conn.execute("DELETE FROM node WHERE id = ?1", [id])? > 0)
    }

    /// Replaces a node's token, which immediately locks out the old one. False
    /// when no node has this id.
    pub fn reset_token(&self, id: i64, token: &str) -> Result<bool> {
        Ok(self.conn().execute("UPDATE node SET token=?2 WHERE id=?1", params![id, token])? > 0)
    }

    pub fn node_by_token(&self, token: &str) -> Result<Option<i64>> {
        Ok(self.conn().query_row("SELECT id FROM node WHERE token = ?1", [token], |r| r.get(0)).optional()?)
    }

    /// Stores the slow-changing facts an agent sends on connect, and reports
    /// whether the node still requires a country lookup for `source`, the address
    /// `agent_ws::country_source` chose. An empty `source` has no country and is
    /// never owed one.
    ///
    /// A new source invalidates the previous country, so the two move together in
    /// one statement: `SET` reads the row as it was, so the comparison is against
    /// the stored address rather than the one being written. The pair replaced
    /// moves to `country_prev_ip` / `country_prev` if it had an answer, and a
    /// source equal to that address takes its answer back without a lookup.
    pub fn save_facts(&self, id: i64, f: &serde_json::Value, ip: &str, source: &str) -> Result<bool> {
        // The same rule `api::agent_register` applies to the name it receives:
        // these values come from an unvouched machine, control characters break
        // the panel's rows, and the length must be bounded. Six of them -- os,
        // kernel, arch, virt, cpu_name, agent_version -- go straight into the
        // anonymous public frame, which is rebuilt and pushed to every viewer
        // every two seconds, so without a ceiling one node would determine that
        // frame's size. 128 rather than 64: a real PRETTY_NAME runs to about 60
        // characters and a CPU model to about 50.
        let s = |k: &str| {
            f.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .chars()
                .filter(|c| !c.is_control())
                .take(128)
                .collect::<String>()
        };
        let n = |k: &str| f.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        let conn = self.conn();
        conn.execute(
            "UPDATE node SET hostname=?2, os=?3, kernel=?4, arch=?5, virt=?6, cpu_name=?7,
                             cpu_cores=?8, mem_total=?9, swap_total=?10, disk_total=?11,
                             agent_version=?12, ip=?13, ipv4=?14, ipv6=?15, country_ip=?16,
                             country=CASE WHEN country_ip=?16 THEN country
                                          WHEN country_prev_ip=?16 THEN country_prev ELSE '' END,
                             country_prev_ip=CASE WHEN country_ip=?16 OR country='' THEN country_prev_ip
                                                  ELSE country_ip END,
                             country_prev=CASE WHEN country_ip=?16 OR country='' THEN country_prev
                                               ELSE country END
             WHERE id=?1",
            params![
                id,
                s("hostname"),
                s("os"),
                s("kernel"),
                s("arch"),
                s("virt"),
                s("cpu_name"),
                n("cpu_cores"),
                n("mem_total"),
                n("swap_total"),
                n("disk_total"),
                s("agent_version"),
                ip,
                s("ipv4"),
                s("ipv6"),
                source
            ],
        )?;
        let blank: bool = conn.query_row("SELECT country = '' FROM node WHERE id=?1", [id], |r| r.get(0))?;
        Ok(blank && !source.is_empty())
    }

    /// Whether the node still lacks a country for `source`: false once a lookup
    /// has landed, or once the node has moved to another address.
    pub fn country_owed(&self, id: i64, source: &str) -> Result<bool> {
        let owed = self
            .conn()
            .query_row(
                "SELECT country = '' FROM node WHERE id=?1 AND country_ip=?2",
                params![id, source],
                |r| r.get(0),
            )
            .optional()?;
        Ok(owed.unwrap_or(false))
    }

    /// Records the country a lookup returned, unless the node moved to another
    /// address while the lookup was outstanding. This is the same rule
    /// `save_facts` encodes in its `CASE`: the country belongs to the address it
    /// was asked about, so a late answer for an address the node has left is not
    /// an answer about the node. Kept apart from the panel's own writes:
    /// `update_node` never touches this column.
    pub fn set_country(&self, id: i64, cc: &str, source: &str) -> Result<()> {
        self.conn()
            .execute("UPDATE node SET country=?2 WHERE id=?1 AND country_ip=?3", params![id, cc, source])?;
        Ok(())
    }

    // ---- traffic ----

    /// Every node's counters in one query, because the node list renders a row per
    /// node and a query per node would queue the agents' writes behind it.
    ///
    /// The period counters are gated on the period they were written for. They
    /// restart lazily in `accumulate`, on the node's next report, so a node
    /// offline since before a boundary still holds the previous period's bytes on
    /// disk. This is the only reader, so the rule lives in one place.
    pub fn all_traffic(&self) -> HashMap<i64, Traffic> {
        let conn = self.conn();
        let Ok(mut stmt) = conn.prepare_cached(
            "SELECT t.node_id, t.total_rx, t.total_tx, t.month_rx, t.month_tx, t.month_start,
                    t.day_rx, t.day_tx, t.day_start, n.traffic_reset_day
                 FROM traffic t JOIN node n ON n.id = t.node_id",
        ) else {
            return HashMap::new();
        };
        let today = Local::now().date_naive();
        let day = today.to_string();
        let rows = stmt.query_map([], |r| {
            // Zero rather than absent: a theme drawing a meter requires a
            // number.
            let current = |stored: String, now: &str, rx: i64, tx: i64| {
                if stored == now {
                    (rx, tx)
                } else {
                    (0, 0)
                }
            };
            let period = period_start(today, r.get(9)?).to_string();
            let (month_rx, month_tx) = current(r.get(5)?, &period, r.get(3)?, r.get(4)?);
            let (day_rx, day_tx) = current(r.get(8)?, &day, r.get(6)?, r.get(7)?);
            Ok((
                r.get::<_, i64>(0)?,
                Traffic {
                    total_rx: r.get(1)?,
                    total_tx: r.get(2)?,
                    month_rx,
                    month_tx,
                    month_start: period,
                    day_rx,
                    day_tx,
                },
            ))
        });
        rows.map(|r| r.flatten().collect()).unwrap_or_default()
    }

    /// The traffic row as stored, without the period gate `all_traffic` applies,
    /// for tests that compare what two ways of booking left behind.
    #[cfg(test)]
    pub fn stored_traffic(&self, node_id: i64) -> Traffic {
        self.conn()
            .query_row(
                "SELECT total_rx, total_tx, month_rx, month_tx, month_start, day_rx, day_tx
                 FROM traffic WHERE node_id=?1",
                [node_id],
                |r| {
                    Ok(Traffic {
                        total_rx: r.get(0)?,
                        total_tx: r.get(1)?,
                        month_rx: r.get(2)?,
                        month_tx: r.get(3)?,
                        month_start: r.get(4)?,
                        day_rx: r.get(5)?,
                        day_tx: r.get(6)?,
                    })
                },
            )
            .expect("a traffic row")
    }

    /// Books one reading of a node's raw kernel counters into its running totals.
    ///
    /// A changed boot_id, or a counter that moved backwards, means the reading no
    /// longer continues the previous one; the total must not follow it downward.
    /// Readings arrive here about once a minute rather than with every report,
    /// which gives the same totals: see `agent_ws::file`.
    ///
    /// `at` is when the hub received the reading, and dates it for the day and
    /// billing period. A reading held back over midnight is booked after it, and
    /// still belongs to the day it arrived in.
    ///
    /// The billing reset day is read here rather than passed in: it is one join
    /// from a row this already reads, and fetching it separately would cost every
    /// booking a second acquisition of the single write connection.
    pub fn accumulate(
        &self,
        node_id: i64,
        boot_id: &str,
        (rx, tx): (i64, i64),
        at: DateTime<Local>,
    ) -> Result<Traffic> {
        let conn = self.conn();
        let (
            prev_boot,
            last_rx,
            last_tx,
            mut total_rx,
            mut total_tx,
            mut month_rx,
            mut month_tx,
            month_start,
            mut day_rx,
            mut day_tx,
            day_start,
            reset_day,
        ) = conn
            .prepare_cached(
                "SELECT t.boot_id, t.last_rx, t.last_tx, t.total_rx, t.total_tx, t.month_rx, t.month_tx,
                    t.month_start, t.day_rx, t.day_tx, t.day_start, n.traffic_reset_day
                 FROM traffic t JOIN node n ON n.id = t.node_id WHERE t.node_id=?1",
            )?
            .query_row([node_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, u32>(11)?,
                ))
            })?;

        // Only bytes this hub observed a counter climb through are booked.
        // Without a baseline under this exact boot there is nothing to subtract
        // from, and a bare reading represents the machine's entire history.
        //
        // The baseline can be missing in three ways, all handled identically. A
        // first reading has none. A reading that shrank under the same boot lost
        // one -- an interface included in the sum has disappeared -- so the
        // reading is the remainder of that history and booking it would count it
        // twice. A changed boot_id means the counters restarted, that the agent
        // now sums a different set of interfaces (it appends a digest of them,
        // so a device joining the sum is caught as well as one leaving it), or,
        // indistinguishably from here, that a second machine shares the token.
        // Realigning costs the seconds since the reboot; the alternative costs
        // hundreds of gigabytes against a total that only increases.
        let (d_rx, d_tx) = if prev_boot.is_empty() || prev_boot != boot_id {
            // Logged in either case: on a healthy node this is a reboot or the
            // agent summing a different set of interfaces, while one every few
            // seconds indicates two machines sharing a token or counted
            // interfaces coming and going. The value stays out of the log: it is
            // the agent's text.
            if !prev_boot.is_empty() {
                info!("node {node_id} reports a new boot_id; re-aligning");
            }
            (0, 0)
        } else {
            ((rx.saturating_sub(last_rx)).max(0), (tx.saturating_sub(last_tx)).max(0))
        };
        // Saturating rather than a plain `+`: the release profile disables
        // overflow checks, so a total near i64::MAX would wrap to a large
        // negative -- a lifetime figure that has decreased. Two paths reach this
        // column: a node's own counters, which arrive from another repository's
        // binary, and `set_traffic`, through which the panel writes corrections.
        // Clamping here covers both rather than each caller separately.
        total_rx = total_rx.saturating_add(d_rx);
        total_tx = total_tx.saturating_add(d_tx);
        month_rx = month_rx.saturating_add(d_rx);
        month_tx = month_tx.saturating_add(d_tx);
        day_rx = day_rx.saturating_add(d_rx);
        day_tx = day_tx.saturating_add(d_tx);

        // Both boundaries are calendar dates -- the day a provider resets an
        // allowance, the day a person means by "today" -- so both follow the
        // hub's local timezone rather than UTC.
        //
        // Never dated before a period the row already carries. The panel stamps
        // the current period with a correction, which a reading held back from
        // before the boundary would otherwise read as a new period and discard.
        // A date later than the stamp is used as it is, so a changed reset day
        // still takes effect. A stamp later than today came from a clock since
        // stepped back and is not honoured: it would hold both counters in that
        // period, which the read side answers as zero, until the date caught up.
        let (date, today) = (at.date_naive(), Local::now().date_naive());
        let dated = |stored: &str| {
            stored
                .parse::<NaiveDate>()
                .ok()
                .filter(|stamp| *stamp <= today)
                .map_or(date, |stamp| stamp.max(date))
        };
        let period = period_start(dated(&month_start), reset_day).to_string();
        if month_start != period {
            // A new billing period restarts the month counter but not the total.
            month_rx = d_rx;
            month_tx = d_tx;
        }
        let day = dated(&day_start).to_string();
        if day_start != day {
            day_rx = d_rx;
            day_tx = d_tx;
        }

        conn.prepare_cached(
            "UPDATE traffic SET boot_id=?2, last_rx=?3, last_tx=?4, total_rx=?5, total_tx=?6,
                            month_rx=?7, month_tx=?8, month_start=?9, day_rx=?10, day_tx=?11,
                            day_start=?12 WHERE node_id=?1",
        )?
        .execute(params![
            node_id, boot_id, rx, tx, total_rx, total_tx, month_rx, month_tx, period, day_rx, day_tx, day
        ])?;
        Ok(Traffic { total_rx, total_tx, month_rx, month_tx, month_start: period, day_rx, day_tx })
    }

    /// Allows the panel to correct a total, for example after moving a node to
    /// new hardware.
    ///
    /// The corrected month figures are stamped with the current period; otherwise
    /// they would belong to whichever period the row still held, `all_traffic`
    /// would read them back as zero, and the node's next report would restart the
    /// counter and discard the correction.
    ///
    /// False when no node has this id.
    pub fn set_traffic(&self, node_id: i64, p: &TrafficPatch) -> Result<bool> {
        let conn = self.conn();
        let Some(reset_day): Option<u32> = conn
            .query_row("SELECT traffic_reset_day FROM node WHERE id=?1", [node_id], |r| r.get(0))
            .optional()?
        else {
            return Ok(false);
        };
        let period = period_start(Local::now().date_naive(), reset_day).to_string();
        conn.execute(
            "UPDATE traffic SET total_rx=COALESCE(?2,total_rx), total_tx=COALESCE(?3,total_tx),
                 month_rx=COALESCE(?4,CASE WHEN month_start=?6 THEN month_rx ELSE 0 END),
                 month_tx=COALESCE(?5,CASE WHEN month_start=?6 THEN month_tx ELSE 0 END), month_start=?6
             WHERE node_id=?1",
            params![node_id, p.total_rx, p.total_tx, p.month_rx, p.month_tx, period],
        )?;
        Ok(true)
    }

    // ---- metrics ----

    pub fn insert_metric(&self, node_id: i64, ts: i64, m: &serde_json::Value) -> Result<()> {
        let f = |k: &str| m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let n = |k: &str| m.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        self.conn()
            .prepare_cached(
                "INSERT OR REPLACE INTO metric
               (node_id, ts, cpu, mem_used, swap_used, disk_used, net_rx, net_tx, tcp, udp, procs)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?
            .execute(params![
                node_id,
                ts,
                f("cpu"),
                n("mem_used"),
                n("swap_used"),
                n("disk_used"),
                n("net_rx"),
                n("net_tx"),
                n("tcp"),
                n("udp"),
                n("procs")
            ])?;
        Ok(())
    }

    /// History for one node, thinned to one sample every `step` seconds.
    ///
    /// Bucketed rather than filtered on a multiple of `step`: rows normally land
    /// on the minute, but nothing enforces it, and a filter would return nothing
    /// for a stamp falling between grid lines.
    ///
    /// Averaged over the bucket rather than sampled from it. Keeping one row per
    /// bucket would reintroduce the 1/60 sampling the write side already rejects:
    /// the seven-day window integrated to 53.69 GB against the 27.52 GB the
    /// minutes hold, while averaging gives 28.02 GB, matching the accumulator.
    ///
    /// `swap_used`, `tcp`, `udp` and `procs` are stored but not returned, as
    /// nothing draws them from history. The columns are retained deliberately;
    /// `load1` was the fifth and has been removed, see `migrate_to_2`.
    ///
    /// The stamp is the bucket's start rather than a row inside it, so every
    /// series lands on one grid and the probe rows below can be shared.
    pub fn metrics(&self, node_id: i64, since: i64, step: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(
            "SELECT (MIN(ts)/?3)*?3, AVG(cpu), CAST(AVG(mem_used) AS INTEGER),
                    CAST(AVG(disk_used) AS INTEGER),
                    CAST(AVG(net_rx) AS INTEGER), CAST(AVG(net_tx) AS INTEGER)
             FROM metric WHERE node_id=?1 AND ts>=?2 GROUP BY ts/?3 ORDER BY ts/?3",
        )?;
        let rows = stmt.query_map(params![node_id, since, step], |r| {
            Ok(serde_json::json!({
                "ts": r.get::<_, i64>(0)?, "cpu": r.get::<_, f64>(1)?,
                "mem_used": r.get::<_, i64>(2)?, "disk_used": r.get::<_, i64>(3)?,
                "net_rx": r.get::<_, i64>(4)?, "net_tx": r.get::<_, i64>(5)?,
            }))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Drops history beyond the retention window. Traffic totals live in their
    /// own table precisely so history can be pruned freely.
    pub fn prune(&self, keep_days: i64) -> Result<usize> {
        let cutoff = Utc::now().timestamp() - keep_days * 86_400;
        let conn = self.conn();
        let a = conn.execute("DELETE FROM metric WHERE ts < ?1", [cutoff])?;
        let b = conn.execute("DELETE FROM ping_record WHERE ts < ?1", [cutoff])?;
        Ok(a + b)
    }

    // ---- ping ----

    pub fn ping_tasks(&self) -> Result<Vec<PingTask>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, name, target, interval, auto_join FROM ping_task ORDER BY sort, id")?;
        let tasks: Vec<PingTask> = stmt
            .query_map([], |r| {
                Ok(PingTask {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    target: r.get(2)?,
                    interval: r.get(3)?,
                    nodes: Vec::new(),
                    auto_join: r.get(4)?,
                    base: None,
                })
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let mut stmt = conn.prepare("SELECT node_id FROM ping_node WHERE task_id=?1")?;
        tasks
            .into_iter()
            .map(|mut t| {
                t.nodes = stmt.query_map([t.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
                Ok(t)
            })
            .collect()
    }

    /// The maximum number of probes one node may be assigned.
    ///
    /// The agent enforces the same limit: `MAX_PING_TASKS` in that repository
    /// caps the list it will run, since a compromised or buggy hub could
    /// otherwise ask a node for hundreds of outbound connects per second. That
    /// cap is a defence and remains, but on its own it truncates silently,
    /// leaving one line in the node's journal while the hub continues pushing
    /// probes that never run and drawing charts that stay empty.
    ///
    /// The hub knows the total, so the hub issues the refusal. The two must stay
    /// in step; the agent's copy is the backstop rather than the message.
    pub const MAX_PROBES_PER_NODE: i64 = 64;

    /// The shortest probe interval in seconds. The agent clamps to the same
    /// floor, so together with [`Self::MAX_PROBES_PER_NODE`] it bounds how many
    /// results an honest node can send.
    pub const MIN_PROBE_INTERVAL: i64 = 5;

    /// Replaces the assignments wholesale, or with `base` applies only what
    /// changed from it. Either way in one transaction: failing between the
    /// deletes and the inserts would unassign nodes from a probe the panel still
    /// lists them under.
    pub fn save_ping_task(&self, t: &PingTask) -> Result<i64> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let id = if t.id > 0 {
            let updated = tx.execute(
                "UPDATE ping_task SET name=?2, target=?3, interval=?4, auto_join=?5 WHERE id=?1",
                params![t.id, t.name, t.target, t.interval, t.auto_join],
            )?;
            // Deleted from another session. Without this the first assignment
            // would fail on the task's foreign key and be reported against a
            // node, or, with none, the save would report success.
            if updated == 0 {
                refuse!("监控不存在，可能已被删除");
            }
            t.id
        } else {
            // At the end, as in `create_node`.
            tx.execute(
                "INSERT INTO ping_task (name, target, interval, auto_join, sort)
                 VALUES (?1,?2,?3,?4,(SELECT COALESCE(MAX(sort),-1)+1 FROM ping_task))",
                params![t.name, t.target, t.interval, t.auto_join],
            )?;
            tx.last_insert_rowid()
        };
        let added: Vec<i64> = match &t.base {
            Some(base) if t.id > 0 => {
                for node in base.iter().filter(|n| !t.nodes.contains(n)) {
                    tx.execute("DELETE FROM ping_node WHERE task_id=?1 AND node_id=?2", params![id, node])?;
                }
                t.nodes.iter().filter(|n| !base.contains(n)).copied().collect()
            }
            _ => {
                tx.execute("DELETE FROM ping_node WHERE task_id=?1", [id])?;
                t.nodes.clone()
            }
        };
        for node in &added {
            // OR IGNORE covers a node that joined while the editor was open and
            // was then ticked. It does not cover the foreign key, which remains
            // the check; naming the node turns SQLite's "FOREIGN KEY constraint
            // failed" into something the panel can show.
            tx.execute(
                "INSERT OR IGNORE INTO ping_node (task_id, node_id) VALUES (?1,?2)",
                params![id, node],
            )
            .map_err(|e| match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::ConstraintViolation) => anyhow::Error::from(e)
                    .context(crate::Shown(format!("服务器 {node} 不存在，可能已被删除"))),
                _ => e.into(),
            })?;
        }
        // Queried from the table after the rows are in rather than counted from
        // the request: an update changes this task's own assignments, so
        // arithmetic on the way in would have to subtract them again. The
        // transaction makes this atomic with the write, and bailing here rolls it
        // back.
        // By name: the panel identifies nodes by name and never shows an id.
        let crowded: Option<String> = tx
            .query_row(
                "SELECT n.name FROM ping_node p JOIN node n ON n.id = p.node_id
                 GROUP BY p.node_id HAVING COUNT(*) > ?1 LIMIT 1",
                [Self::MAX_PROBES_PER_NODE],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(node) = crowded {
            refuse!(
                "服务器「{node}」会被分配超过 {} 个探测任务，agent 最多只跑这么多，多出来的会被静默丢掉",
                Self::MAX_PROBES_PER_NODE
            );
        }
        // The next node created receives every auto-joining probe at once.
        let joining: i64 =
            tx.query_row("SELECT COUNT(*) FROM ping_task WHERE auto_join", [], |r| r.get(0))?;
        if joining > Self::MAX_PROBES_PER_NODE {
            refuse!(
                "新服务器会自动加入 {joining} 个探测任务，agent 最多只跑 {} 个",
                Self::MAX_PROBES_PER_NODE
            );
        }
        tx.commit()?;
        Ok(id)
    }

    /// Deletes a probe and the results filed under it.
    ///
    /// `ping_record` carries no foreign key -- it is WITHOUT ROWID and keyed for
    /// the chart query -- so it is cleared explicitly, as in `delete_node`.
    /// SQLite reassigns a deleted probe's id to the next one created, and the
    /// chart selects on `task_id IN (assignments for this node)`: without this
    /// the new probe would draw the removed one's latency under its own name,
    /// with its timeouts folded into the loss figure.
    ///
    /// The delete is a scan -- the key begins at `node_id` -- comparable in cost
    /// to `prune`, for an action taken manually a few times a year.
    pub fn delete_ping_task(&self, id: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM ping_record WHERE task_id = ?1", [id])?;
        conn.execute("DELETE FROM ping_task WHERE id=?1", [id])?;
        Ok(())
    }

    /// The task list pushed to one agent.
    ///
    /// Ordered, because the agent keeps the first [`Self::MAX_PROBES_PER_NODE`]
    /// as its backstop against a hub requesting hundreds. Unordered, a list at
    /// that boundary could yield a different subset on each push, restarting half
    /// the timers each time; `save_ping_task` prevents reaching that boundary,
    /// and this makes the backstop deterministic should a database arrive there
    /// by another route.
    ///
    /// By id rather than the panel's `sort`, so reordering in the panel changes
    /// nothing an agent holds and needs no push.
    pub fn ping_tasks_for(&self, node_id: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.target, t.interval FROM ping_task t
             JOIN ping_node n ON n.task_id = t.id WHERE n.node_id = ?1 ORDER BY t.id",
        )?;
        let rows = stmt.query_map([node_id], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, i64>(0)?, "target": r.get::<_, String>(1)?,
                "interval": r.get::<_, i64>(2)?
            }))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Probe names keyed by id, for labelling one node's latency chart. Names
    /// only: targets and node assignments remain behind `Admin`.
    ///
    /// Restricted to the probes assigned to that node, the only ones its chart
    /// has samples to label. A probe name is operator-supplied text that
    /// routinely carries a hostname or a customer, and the rest of the table
    /// belongs to nodes this caller may not be able to see.
    pub fn ping_task_names(&self, node_id: i64) -> Result<serde_json::Value> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name FROM ping_task WHERE id IN (SELECT task_id FROM ping_node WHERE node_id=?1)",
        )?;
        let rows = stmt.query_map([node_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut names = serde_json::Map::new();
        for row in rows {
            let (id, name) = row?;
            names.insert(id.to_string(), serde_json::json!(name));
        }
        Ok(serde_json::Value::Object(names))
    }

    /// Files a node's probe results as `(task_id, ts, latency)`, each only under a
    /// probe this node is assigned. A result for anything else is dropped rather
    /// than treated as an error, since the agent can do nothing useful with the
    /// distinction.
    ///
    /// One transaction for the batch: the session gathers a minute of results and
    /// files them together, since each commit writes at least one page.
    ///
    /// The assignment is tested inside the statement because that is the only
    /// place it is atomic with the write: `ping_record` carries no foreign key,
    /// being WITHOUT ROWID and keyed for the chart query. Two cases arrive
    /// without an assignment. A result already in flight when the panel deleted
    /// its probe, which would otherwise land after `delete_ping_task` swept the
    /// history and be inherited by whichever probe SQLite assigns the id to next.
    /// And a node token in the wrong hands: `task_id` is chosen by the reporter,
    /// so without the test the rows it could create would be unbounded.
    ///
    /// The chart's `task_id IN (assignments)` filter hides both afterwards, but
    /// does not prevent the write, its storage, or the id being reused.
    pub fn insert_pings(&self, node_id: i64, results: &[(i64, i64, i64)]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO ping_record (node_id, task_id, ts, latency)
                 SELECT ?1, ?2, ?3, ?4
                 WHERE EXISTS (SELECT 1 FROM ping_node WHERE task_id = ?2 AND node_id = ?1)",
            )?;
            for (task_id, ts, latency) in results {
                insert.execute(params![node_id, task_id, ts, latency])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Probe results for one node, one sample per probe per `step` seconds: the
    /// bucket's median round trip, its range, and the proportion lost.
    ///
    /// These stamps fall wherever the probe finished rather than on a minute, so
    /// the thinning buckets them instead of matching a multiple, as in `metrics`
    /// above. This is the larger half of that response, since a probe reports far
    /// more often than once a minute.
    ///
    /// [`PING_ROWS`] returns rows in time order, so a bucket is complete the
    /// moment the next opens and only one is held at a time -- at most the probes
    /// assigned to the node times the results one bucket spans.
    ///
    /// Returns the buckets and, alongside them, the proportion of the whole
    /// window each probe lost. The latter cannot be recovered from the former:
    /// [`close_bucket`] divides within each bucket and keeps only the quotient,
    /// so averaging those percentages would weight a bucket holding one sample
    /// equally with one holding twelve. The buckets are necessarily unequal --
    /// the window's first and last are partial by construction, and a probe that
    /// starts, stops, loses its node or skips a round produces more. The
    /// denominators are available only here, in the pass that already reads every
    /// row. Probes that lost nothing are omitted, as `loss` is per bucket.
    pub fn ping_records(
        &self,
        node_id: i64,
        since: i64,
        step: i64,
    ) -> Result<(Vec<serde_json::Value>, serde_json::Value)> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached(PING_ROWS)?;
        let mut rows = stmt.query(params![node_id, since, step])?;
        let mut out = Vec::new();
        // Per probe in the bucket being filled: what answered, and how many did
        // not.
        let mut open: Vec<(i64, Vec<i64>, i64)> = Vec::new();
        // Per probe across the whole window: how many were lost, out of how many.
        // Folded in the same pass rather than queried from SQLite a second time,
        // for the same reason the bucket fold itself is in Rust.
        let mut totals: HashMap<i64, (i64, i64)> = HashMap::new();
        let mut bucket = 0;
        while let Some(row) = rows.next()? {
            let (b, task, latency) = (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?);
            if b != bucket {
                close_bucket(&mut out, &mut open, bucket * step);
                bucket = b;
            }
            let seen = totals.entry(task).or_insert((0, 0));
            seen.1 += 1;
            let probe = match open.iter().position(|(id, ..)| *id == task) {
                Some(at) => &mut open[at],
                None => {
                    open.push((task, Vec::new(), 0));
                    open.last_mut().expect("just pushed")
                }
            };
            // A timeout is stored as -1: excluded from the median and counted
            // instead.
            if latency < 0 {
                probe.2 += 1;
                seen.0 += 1;
            } else {
                probe.1.push(latency);
            }
        }
        close_bucket(&mut out, &mut open, bucket * step);
        // Probe by probe in the panel's order, each probe's rows still in time
        // order. Themes take their series, colours and legend from the order in
        // which probes first appear; bucket by bucket, that would be whichever
        // probe happened to answer inside the window's partial first bucket.
        let rank: HashMap<i64, usize> = conn
            .prepare_cached("SELECT id FROM ping_task ORDER BY sort, id")?
            .query_map([], |r| r.get(0))?
            .enumerate()
            .map(|(i, id)| id.map(|id| (id, i)))
            .collect::<Result<_, _>>()?;
        // Sorted after releasing the connection the agents write through. A
        // probe missing from the rank, which the assignment filter in
        // `PING_ROWS` rules out today, goes last rather than taking the first
        // colour.
        drop(rows);
        drop(stmt);
        drop(conn);
        out.sort_by_cached_key(|row| {
            row["task_id"].as_i64().and_then(|id| rank.get(&id).copied()).unwrap_or(usize::MAX)
        });
        // Unrounded: the caller decides how to render it, and rounding here would
        // turn 0.14% into the 0% that denotes no loss at all.
        let loss: serde_json::Map<String, serde_json::Value> = totals
            .into_iter()
            .filter(|(_, (lost, _))| *lost > 0)
            .map(|(task, (lost, samples))| {
                (task.to_string(), serde_json::json!(100.0 * lost as f64 / samples as f64))
            })
            .collect();
        Ok((out, serde_json::Value::Object(loss)))
    }

    // ---- the database file itself ----

    /// The file this connection is open on, empty for `:memory:`.
    pub fn file(&self) -> String {
        main_file(&self.conn())
    }

    /// The retention window used by both `prune` and the data page. Stored as
    /// text by the settings form, so a missing or unparsable value falls back to
    /// the default rather than erroring.
    pub fn retention_days(&self) -> i64 {
        self.get("retention_days").and_then(|v| v.parse::<i64>().ok()).unwrap_or(7).clamp(1, 3_650)
    }

    /// What the panel's data page reads: how much space the file occupies, how
    /// much of that is free pages awaiting a `VACUUM`, and how far back the
    /// history actually reaches.
    ///
    /// `oldest` against `retention` is the one pair here that can indicate a
    /// fault: history older than the window means `prune` has not been running.
    pub fn stats(&self) -> Result<serde_json::Value> {
        // Before acquiring the connection: `conn()` returns a guard on a plain
        // Mutex, and `retention_days` acquires the same one.
        let retention = self.retention_days();
        let conn = self.conn();
        let file = main_file(&conn);
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let free_pages: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        // Both are pruned at the same cutoff, so the earlier of the two marks
        // where history begins. A full scan of each, which the counts below
        // already incur.
        let oldest: Option<i64> = conn.query_row(
            "SELECT MIN(ts) FROM (SELECT MIN(ts) AS ts FROM metric UNION ALL SELECT MIN(ts) FROM ping_record)",
            [],
            |r| r.get(0),
        )?;
        let mut rows = serde_json::Map::new();
        for table in TABLES {
            let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            rows.insert(table.to_owned(), serde_json::json!(n));
        }
        Ok(serde_json::json!({
            "path": file,
            "size": bytes_of(&file),
            "wal": bytes_of(&format!("{file}-wal")),
            "free": free_pages * page_size,
            "oldest": oldest,
            "retention": retention,
            "rows": rows,
        }))
    }

    /// Writes a consistent copy of the live database to `dest`, which must not
    /// already exist.
    ///
    /// `VACUUM INTO` is SQLite's own mechanism for this: one statement, a single
    /// read transaction, and a compacted copy with free pages already dropped. It
    /// reads the whole file, so the caller runs it off the runtime -- every other
    /// statement here is sub-millisecond, this one is not.
    pub fn backup_into(&self, dest: &str) -> Result<()> {
        // A second connection to the same file. `VACUUM INTO` only reads, and WAL
        // allows it to read a consistent snapshot while the agents continue
        // writing through the first -- exporting is the one heavy operation here
        // that need not block them. A fresh connection inherits none of the
        // PRAGMAs in SCHEMA, so the busy timeout must be set again or a
        // checkpoint racing this read returns SQLITE_BUSY immediately.
        let reader = Connection::open(self.file())?;
        reader.busy_timeout(std::time::Duration::from_secs(5))?;
        reader.execute("VACUUM INTO ?1", [dest])?;
        // The copy is the credential store in one portable file: node tokens in
        // the clear, the GitHub secret, the password hash. SQLite creates it
        // under the umask, which at the usual 022 is world-readable.
        own_only(dest);
        Ok(())
    }

    /// Rebuilds the file, reclaiming the pages deleted history left behind.
    /// Returns the bytes recovered.
    ///
    /// SQLite's constraints on `VACUUM`, and why they hold here: it cannot run
    /// inside a transaction or with a live statement on the connection (there is
    /// one connection, and this call owns it); it requires roughly as much free
    /// disk as the database itself, and a failure rolls back leaving the original
    /// untouched; and it can renumber rowids, which nothing here keys on, since
    /// `metric` and `ping_record` are WITHOUT ROWID and every other table
    /// declares its own primary key.
    ///
    /// In WAL mode the rewrite lands in the WAL first, so without the checkpoint
    /// the file on disk grows rather than shrinking.
    pub fn vacuum(&self) -> Result<i64> {
        let conn = self.conn();
        let file = main_file(&conn);
        let before = on_disk(&file);
        conn.execute_batch("VACUUM")?;
        // Best effort: the space is already reclaimed within the database, and a
        // checkpoint that cannot run now does not constitute a failed vacuum.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        Ok((before - on_disk(&file)).max(0))
    }

    /// What a file must satisfy before a single page of it is copied over the
    /// live database. Restore is the one operation here that destroys data, and
    /// the file behind it originates from a disk this hub knows nothing about.
    ///
    /// **Writes to `src`.** The migrations an older backup requires run here, on
    /// the upload, rather than after copying: everything that can fail does so
    /// while the live database is still untouched. The caller owns that file and
    /// deletes it in either case.
    pub fn check_backup(&self, src: &str) -> Result<()> {
        const NOT_A_BACKUP: &str = "这不是 hub 导出的备份文件";
        // Read-write rather than read-only: a plain copy of a running hub's
        // database is in WAL mode, and SQLite cannot open such a file read-only
        // without its -shm companion.
        let candidate = Connection::open(src)?;
        let health: String = candidate
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .context(crate::Shown(NOT_A_BACKUP.into()))?;
        if health != "ok" {
            info!("uploaded backup fails integrity_check: {health}");
            refuse!("备份文件已损坏，数据库完整性检查没有通过");
        }
        // Pages are copied verbatim, so whatever schema the file carries becomes
        // the schema this hub runs its statements against. A view or trigger
        // where a table belongs would route every subsequent write through
        // externally supplied code.
        let plotted: i64 = candidate.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('view', 'trigger')",
            [],
            |r| r.get(0),
        )?;
        if plotted > 0 {
            refuse!("文件里有视图或触发器，不是 hub 导出的备份");
        }
        let version: i64 = candidate.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            refuse!("备份来自更新版本的 hub（数据库版本 {version}，这台只认到 {SCHEMA_VERSION}），先升级 hub 再恢复");
        }
        let required_tables: &[&str] = match version {
            v if v < 10 => &LEGACY_TABLES,
            10 => &V10_TABLES,
            11 => &V11_TABLES,
            12 => &V12_TABLES,
            _ => &TABLES,
        };
        for table in required_tables {
            let found: i64 = candidate.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )?;
            if found == 0 {
                refuse!("{NOT_A_BACKUP}：缺少 {table} 表");
            }
        }
        // The online backup API refuses a page size change while the destination
        // is in WAL mode; an explicit message is clearer than SQLITE_READONLY.
        let theirs: i64 = candidate.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let ours: i64 = self.conn().query_row("PRAGMA page_size", [], |r| r.get(0))?;
        if theirs != ours {
            refuse!("备份的页大小是 {theirs} 字节，这台 hub 是 {ours} 字节，无法恢复");
        }
        // Brought up to this build's schema here, on the upload. Run after the
        // copy instead, a failed migration would leave the hub on a database it
        // could not use while reporting a failure to the panel -- the one
        // arrangement in which the restore has failed and the original data is
        // also gone.
        migrate(&candidate, version)?;
        // The migration lands in a -wal beside a backup taken from a running hub.
        // Checkpointed here so the copy below reads a single file.
        let _ = candidate.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));

        // Table names are not a schema. Pages are copied verbatim, so the columns
        // the file carries become the ones this hub's statements run against, and
        // correctly named tables holding the wrong columns pass every gate
        // above while leaving the database unusable.
        //
        // Compared against a database this build creates for itself, so there is
        // no second column list to keep in step with `SCHEMA`. Names are compared
        // as sets rather than as stored DDL: a migrated old backup reaches the
        // same columns through `ALTER TABLE`, whose text never matches a fresh
        // `CREATE TABLE`. Extra columns are ignored.
        let reference = Connection::open_in_memory()?;
        reference.execute_batch(SCHEMA)?;
        migrate(&reference, 0)?;
        for table in TABLES {
            let want = columns_of(&reference, table)?;
            let got = columns_of(&candidate, table)?;
            let mut missing: Vec<&str> = want.difference(&got).map(String::as_str).collect();
            if !missing.is_empty() {
                missing.sort_unstable();
                refuse!("{NOT_A_BACKUP}：{table} 表缺少字段 {}", missing.join("、"));
            }
        }
        Ok(())
    }

    /// Copies a checked backup over the live database page by page through
    /// SQLite's online backup API: the destination retains its file, permissions
    /// and journal mode, and a partial failure rolls back rather than leaving
    /// half a database behind.
    ///
    /// Call [`Db::check_backup`] first, as it is what brings `src` to this
    /// build's schema; the copy is then the last step and nothing after it can
    /// fail. Like the other two, this reads and writes the whole file and belongs
    /// off the runtime.
    pub fn restore_from(&self, src: &str) -> Result<()> {
        let mut conn = self.conn();
        conn.restore(rusqlite::MAIN_DB, src, None::<fn(rusqlite::backup::Progress)>)?;
        Ok(())
    }

    // ---- sessions ----

    pub fn create_session(&self, token_hash: &str, expires_at: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO session (token_hash, expires_at) VALUES (?1, ?2)",
            params![token_hash, expires_at],
        )?;
        Ok(())
    }

    pub fn session_valid(&self, token_hash: &str) -> bool {
        self.conn()
            .query_row(
                "SELECT 1 FROM session WHERE token_hash=?1 AND expires_at > ?2",
                params![token_hash, Utc::now().timestamp()],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Live sessions, newest first. Expired rows are filtered here rather than
    /// left to `expire_sessions`, which sweeps only once an hour.
    pub fn sessions(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT token_hash, expires_at FROM session WHERE expires_at > ?1 ORDER BY expires_at DESC",
        )?;
        let rows = stmt
            .query_map([Utc::now().timestamp()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn drop_session(&self, token_hash: &str) -> Result<()> {
        self.conn().execute("DELETE FROM session WHERE token_hash=?1", [token_hash])?;
        Ok(())
    }

    /// Replaces the admin password hash and signs every session out, both or
    /// neither: a reset that stored the hash and then failed would report failure
    /// while the old password no longer works.
    pub fn replace_password(&self, hash: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO setting (key, value) VALUES ('admin_password_hash', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [hash],
        )?;
        tx.execute("DELETE FROM session", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Invalidates every login. Used after a restore, which would otherwise
    /// revive every session the backup holds.
    pub fn drop_all_sessions(&self) -> Result<()> {
        self.conn().execute("DELETE FROM session", [])?;
        Ok(())
    }

    pub fn expire_sessions(&self) -> Result<()> {
        self.conn().execute("DELETE FROM session WHERE expires_at <= ?1", [Utc::now().timestamp()])?;
        Ok(())
    }
}

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
fn close_bucket(out: &mut Vec<serde_json::Value>, open: &mut Vec<(i64, Vec<i64>, i64)>, ts: i64) {
    for (task, mut answered, lost) in open.drain(..) {
        answered.sort_unstable();
        let middle = match answered.len() {
            0 => None,
            n if n % 2 == 1 => Some(answered[n / 2]),
            n => Some((answered[n / 2 - 1] + answered[n / 2]) / 2),
        };
        let mut row = serde_json::json!({"task_id": task, "ts": ts, "latency": middle});
        // Only when the bucket actually varied. At the hour and six-hour windows a
        // bucket holds one sample, and a band would be a zero-height ribbon under
        // every line.
        if let (Some(lo), Some(hi)) = (answered.first(), answered.last()) {
            if hi > lo {
                row["band"] = serde_json::json!([lo, hi]);
            }
        }
        if lost > 0 {
            let total = answered.len() as i64 + lost;
            row["loss"] = ((100 * lost + total - 1) / total).into();
        }
        out.push(row);
    }
}

fn row_to_node(r: &rusqlite::Row<'_>) -> Node {
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

fn row_to_proxy_instance(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProxyInstance> {
    Ok(ProxyInstance {
        id: r.get("id")?,
        node_id: r.get("node_id")?,
        engine: r.get("engine")?,
        version: r.get("version")?,
        status: r.get("status")?,
        config_hash: r.get("config_hash")?,
        config_version: r.get("config_version")?,
        config_updated_at: r.get("config_updated_at")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn row_to_proxy_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProxyUser> {
    Ok(ProxyUser {
        id: r.get("id")?,
        name: r.get("name")?,
        uuid: r.get("uuid")?,
        enabled: r.get("enabled")?,
        created_at: r.get("created_at")?,
        note: r.get("note")?,
        updated_at: r.get("updated_at")?,
        proxy_node_ids: Vec::new(),
    })
}

fn query_proxy_user(conn: &Connection, id: i64) -> Result<Option<ProxyUser>> {
    Ok(conn
        .query_row(
            "SELECT id, username AS name, uuid, enabled, created_at, note, updated_at
             FROM proxy_user WHERE id=?1",
            [id],
            row_to_proxy_user,
        )
        .optional()?)
}

fn proxy_user_node_ids(conn: &Connection, user_id: i64) -> Result<Vec<i64>> {
    let mut stmt =
        conn.prepare("SELECT proxy_node_id FROM proxy_user_node WHERE user_id=?1 ORDER BY proxy_node_id")?;
    let ids = stmt.query_map([user_id], |row| row.get::<_, i64>(0))?;
    Ok(ids.collect::<Result<_, _>>()?)
}

fn validated_proxy_node_ids(conn: &Connection, requested: &[i64]) -> Result<Vec<i64>> {
    let ids = requested.iter().copied().collect::<std::collections::BTreeSet<_>>();
    if ids.iter().any(|id| *id <= 0) {
        refuse!("代理节点 ID 无效");
    }
    for id in &ids {
        let exists = conn.query_row("SELECT EXISTS(SELECT 1 FROM proxy_node WHERE id=?1)", [id], |row| {
            row.get::<_, bool>(0)
        })?;
        if !exists {
            refuse!("所选代理节点不存在");
        }
    }
    Ok(ids.into_iter().collect())
}

fn proxy_server_ids_for_nodes(conn: &Connection, proxy_node_ids: &[i64]) -> Result<Vec<i64>> {
    let mut server_ids = std::collections::BTreeSet::new();
    for proxy_node_id in proxy_node_ids {
        if let Some(node_id) = conn
            .query_row("SELECT node_id FROM proxy_node WHERE id=?1", [proxy_node_id], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
        {
            server_ids.insert(node_id);
        }
    }
    Ok(server_ids.into_iter().collect())
}

fn validate_proxy_user_name_note(name: &str, note: &str) -> Result<()> {
    if name.trim().is_empty() {
        refuse!("用户名称不能为空");
    }
    if name.trim().chars().count() > 120 {
        refuse!("用户名称不能超过 120 个字符");
    }
    if note.chars().count() > 2_000 {
        refuse!("备注不能超过 2000 个字符");
    }
    Ok(())
}

fn validate_proxy_user_profile(name: &str, note: &str, uuid: &str) -> Result<()> {
    validate_proxy_user_name_note(name, note)?;
    if !crate::proxy_config::is_uuid(uuid) {
        refuse!("代理用户 UUID 格式无效");
    }
    Ok(())
}

fn ensure_proxy_user_name_available(conn: &Connection, name: &str, except_id: Option<i64>) -> Result<()> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM proxy_user WHERE username=?1 AND (?2 IS NULL OR id!=?2))",
        params![name.trim(), except_id],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        refuse!("代理用户名已存在");
    }
    Ok(())
}

fn row_to_proxy_node(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProxyNode> {
    Ok(ProxyNode {
        id: r.get("id")?,
        node_id: r.get("node_id")?,
        name: r.get("name")?,
        enabled: r.get("enabled")?,
        protocol: r.get("protocol")?,
        address_mode: r.get("address_mode")?,
        custom_address: r.get("custom_address")?,
        listen_port: r.get("listen_port")?,
        uuid: r.get("uuid")?,
        reality_private_key: r.get("reality_private_key")?,
        reality_public_key: r.get("reality_public_key")?,
        reality_short_id: r.get("reality_short_id")?,
        reality_server_name: r.get("reality_server_name")?,
        reality_dest: r.get("reality_dest")?,
        listen_address: r.get("listen_address")?,
        source_inbound_tag: r.get("source_inbound_tag")?,
        deploy_status: r.get("deploy_status")?,
        last_error: r.get("last_error")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open(":memory:").unwrap()
    }

    fn proxy_node_config(node_id: i64, listen_port: u16) -> ProxyNodeConfig {
        ProxyNodeConfig {
            node_id,
            name: "VLESS Reality".into(),
            enabled: true,
            protocol: "vless_reality".into(),
            address_mode: "ipv4".into(),
            custom_address: None,
            listen_port,
            uuid: "test-uuid".into(),
            reality_private_key: "private-secret".into(),
            reality_public_key: "public-key".into(),
            reality_short_id: "1234abcd".into(),
            reality_server_name: "example.com".into(),
            reality_dest: "example.com:443".into(),
        }
    }

    #[test]
    fn proxy_instance_observations_track_hash_revisions_and_cascade_with_nodes() {
        let db = db();
        let node_id =
            db.create_node(&Node { name: "proxy".into(), ..Default::default() }, "proxy-token").unwrap();
        let first_hash = "a".repeat(64);
        let next_hash = "b".repeat(64);

        db.observe_singbox(node_id, Some("running"), Some(Some("1.12.0")), Some(&first_hash), Some(100), 200)
            .unwrap();
        let first = db.proxy_instances().unwrap().remove(0);
        assert_eq!(first.engine, "singbox");
        assert_eq!(first.status, "running");
        assert_eq!(first.version.as_deref(), Some("1.12.0"));
        assert_eq!(first.config_version, 1);
        assert_eq!(first.config_updated_at, Some(100));

        db.observe_singbox(node_id, None, None, Some(&first_hash), Some(110), 210).unwrap();
        assert_eq!(db.proxy_instances().unwrap()[0].config_version, 1);
        db.observe_singbox(node_id, Some("stopped"), Some(None), Some(&next_hash), Some(120), 220).unwrap();
        let changed = db.proxy_instances().unwrap().remove(0);
        assert_eq!(changed.status, "stopped");
        assert_eq!(changed.version, None);
        assert_eq!(changed.config_hash.as_deref(), Some(next_hash.as_str()));
        assert_eq!(changed.config_version, 2);

        db.delete_node(node_id).unwrap();
        assert!(db.proxy_instances().unwrap().is_empty());
    }

    #[test]
    fn proxy_users_have_unique_identity_and_complete_db_crud() {
        let db = db();
        let uuid = "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e";
        let (created, _) = db.create_proxy_user_with_nodes("alice", uuid, true, "", &[]).unwrap();
        let id = created.id;
        assert_eq!(db.proxy_user(id).unwrap().unwrap().name, "alice");
        assert!(db
            .create_proxy_user_with_nodes("alice", "aa8c947a-1dbd-4905-bcb1-71728c58effb", true, "", &[],)
            .is_err());
        assert!(db.create_proxy_user_with_nodes("bob", uuid, true, "", &[]).is_err());
        assert!(db
            .create_proxy_user_with_nodes(" ", "e2b4b1d8-0af6-40f8-910b-cabfb913a7bc", true, "", &[])
            .is_err());

        let (updated, _) = db.update_proxy_user_profile(id, "alice-2", false, "", &[]).unwrap().unwrap();
        let loaded = db.proxy_users().unwrap().remove(0);
        assert_eq!(loaded.name, "alice-2");
        assert!(!loaded.enabled);
        assert_eq!(updated.uuid, uuid, "普通资料编辑不允许改变 UUID");
        assert!(db.delete_proxy_user(id).unwrap());
        assert!(db.proxy_user(id).unwrap().is_none());
    }

    #[test]
    fn proxy_user_node_assignments_are_transactional_and_cascade() {
        let db = db();
        let servers = ["u-server-a", "u-server-b", "u-server-c"]
            .map(|token| db.create_node(&Node { name: token.into(), ..Default::default() }, token).unwrap());
        let proxy_nodes = servers
            .iter()
            .enumerate()
            .map(|(index, server_id)| {
                db.create_proxy_node(&proxy_node_config(*server_id, 24_000 + index as u16)).unwrap().id
            })
            .collect::<Vec<_>>();
        let uuid = "f15aec0b-10d2-4794-b07a-64c817f6cabe";
        let (created, created_servers) = db
            .create_proxy_user_with_nodes(
                "Alice",
                uuid,
                true,
                "owner",
                &[proxy_nodes[1], proxy_nodes[0], proxy_nodes[1]],
            )
            .unwrap();
        assert_eq!(created.proxy_node_ids, proxy_nodes[..2]);
        assert_eq!(created.note, "owner");
        assert_eq!(created_servers, servers[..2]);
        assert_eq!(db.proxy_users_for_node(servers[0]).unwrap()[0].id, created.id);
        assert!(db.create_proxy_user_with_nodes("Bob", uuid, true, "", &[]).is_err());
        assert!(db
            .create_proxy_user_with_nodes(
                "Invalid target",
                "a0f81cec-73c5-4eb8-a2e2-cd1544946e8e",
                true,
                "",
                &[i64::MAX],
            )
            .is_err());
        assert_eq!(db.proxy_users().unwrap().len(), 1, "无效授权不会留下孤立用户");

        let (updated, affected_servers) = db
            .update_proxy_user_profile(
                created.id,
                "Alice 2",
                false,
                "paused",
                &[proxy_nodes[1], proxy_nodes[2]],
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.proxy_node_ids, proxy_nodes[1..]);
        assert!(!updated.enabled);
        assert_eq!(updated.note, "paused");
        assert_eq!(affected_servers, servers, "旧、新授权服务器合并去重");

        assert!(db.delete_proxy_node(proxy_nodes[1]).unwrap());
        assert_eq!(db.proxy_user(created.id).unwrap().unwrap().proxy_node_ids, [proxy_nodes[2]]);
        assert!(db.delete_proxy_user(created.id).unwrap());
        assert_eq!(
            db.conn()
                .query_row("SELECT COUNT(*) FROM proxy_user_node", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0,
            "删除用户级联清理授权关系"
        );
    }

    #[test]
    fn proxy_user_resync_includes_failed_servers_after_an_assignment_was_removed() {
        let db = db();
        let first_server =
            db.create_node(&Node { name: "old-server".into(), ..Default::default() }, "old").unwrap();
        let current_server =
            db.create_node(&Node { name: "current-server".into(), ..Default::default() }, "current").unwrap();
        let old_proxy = db.create_proxy_node(&proxy_node_config(first_server, 24_111)).unwrap();
        let current_proxy = db.create_proxy_node(&proxy_node_config(current_server, 24_112)).unwrap();
        let user = db
            .create_proxy_user_with_nodes(
                "Alice",
                "f15aec0b-10d2-4794-b07a-64c817f6cabe",
                true,
                "",
                &[current_proxy.id],
            )
            .unwrap()
            .0;

        db.set_proxy_nodes_deploy_status(first_server, "failed", Some("Agent 离线")).unwrap();
        assert_eq!(
            db.proxy_user_server_ids(user.id).unwrap(),
            [first_server, current_server],
            "重试必须包含已取消授权但仍未同步的旧服务器"
        );

        db.set_proxy_nodes_deploy_status(first_server, "deployed", None).unwrap();
        assert_eq!(db.proxy_user_server_ids(user.id).unwrap(), [current_server]);
        assert_eq!(db.proxy_node(old_proxy.id).unwrap().unwrap().node_id, first_server);
    }

    #[test]
    fn proxy_nodes_have_independent_crud_constraints_and_cascade() {
        let db = db();
        let server = db
            .create_node(&Node { name: "server-a".into(), ..Default::default() }, "server-a-token")
            .unwrap();
        let other_server = db
            .create_node(&Node { name: "server-b".into(), ..Default::default() }, "server-b-token")
            .unwrap();

        let mut config = proxy_node_config(server, 443);
        config.name = "  reality-a  ".into();
        let created = db.create_proxy_node(&config).unwrap();
        assert_eq!(created.node_id, server);
        assert_eq!(created.name, "reality-a");
        assert_eq!(created.protocol, "vless_reality");
        assert_eq!(created.deploy_status, "not_deployed");
        assert_eq!(created.last_error, None);
        assert_eq!(created.reality_private_key, "private-secret");
        assert_eq!(db.proxy_node(created.id).unwrap().unwrap().uuid, "test-uuid");

        let duplicate_port = db.create_proxy_node(&proxy_node_config(server, 443)).err().unwrap();
        assert!(!duplicate_port.to_string().contains("private-secret"));
        let on_other_server = db.create_proxy_node(&proxy_node_config(other_server, 443)).unwrap();
        assert_eq!(on_other_server.listen_port, created.listen_port);
        assert_eq!(db.proxy_nodes().unwrap().len(), 2);

        let mut bad = proxy_node_config(server, 8443);
        bad.protocol = "xray".into();
        assert!(db.create_proxy_node(&bad).err().unwrap().downcast_ref::<crate::Shown>().is_some());
        bad.protocol = "vless_reality".into();
        bad.address_mode = "automatic".into();
        assert!(db.create_proxy_node(&bad).err().unwrap().downcast_ref::<crate::Shown>().is_some());
        bad.address_mode = "ipv4".into();
        bad.listen_port = 0;
        assert!(db.create_proxy_node(&bad).err().unwrap().downcast_ref::<crate::Shown>().is_some());
        bad.listen_port = 8443;
        bad.address_mode = "custom".into();
        assert!(db.create_proxy_node(&bad).err().unwrap().downcast_ref::<crate::Shown>().is_some());

        db.conn()
            .execute(
                "UPDATE proxy_node SET deploy_status='deployed', last_error='old error', updated_at=0 WHERE id=?1",
                [created.id],
            )
            .unwrap();
        let update = ProxyNodeUpdate {
            name: "reality-updated".into(),
            enabled: false,
            address_mode: "custom".into(),
            custom_address: Some(" edge.example.net ".into()),
            listen_port: 8443,
            reality_server_name: "www.apple.com".into(),
            reality_dest: "www.apple.com:443".into(),
        };
        assert!(db.update_proxy_node(created.id, &update).unwrap());
        let updated = db.proxy_node(created.id).unwrap().unwrap();
        assert_eq!(updated.name, "reality-updated");
        assert_eq!(updated.address_mode, "custom");
        assert_eq!(updated.custom_address.as_deref(), Some("edge.example.net"));
        assert_eq!(updated.deploy_status, "not_deployed");
        assert_eq!(updated.last_error, None);
        assert!(updated.updated_at > 0);
        assert!(!updated.enabled);
        assert_eq!(updated.node_id, server);
        assert_eq!(updated.uuid, "test-uuid");
        assert_eq!(updated.reality_private_key, "private-secret");
        assert_eq!(updated.reality_public_key, "public-key");
        assert_eq!(updated.reality_short_id, "1234abcd");

        assert!(db
            .regenerate_proxy_node_credentials(
                created.id,
                Some("new-uuid"),
                Some("new-private"),
                Some("new-public"),
                Some("new-short-id"),
            )
            .unwrap());
        let regenerated = db.proxy_node(created.id).unwrap().unwrap();
        assert_eq!(regenerated.uuid, "new-uuid");
        assert_eq!(regenerated.reality_private_key, "new-private");
        assert_eq!(regenerated.reality_public_key, "new-public");
        assert_eq!(regenerated.reality_short_id, "new-short-id");
        assert_eq!(regenerated.deploy_status, "not_deployed");

        assert!(db.delete_proxy_node(created.id).unwrap());
        assert!(!db.delete_proxy_node(created.id).unwrap());
        assert!(db.proxy_node(created.id).unwrap().is_none());

        db.delete_node(other_server).unwrap();
        assert!(db.proxy_nodes().unwrap().is_empty());
    }

    #[test]
    fn imported_proxy_nodes_keep_source_and_listen_private_to_the_database() {
        let db = db();
        let server = db
            .create_node(&Node { name: "import-server".into(), ..Default::default() }, "import-token")
            .unwrap();
        let imported = db
            .create_imported_proxy_node(&proxy_node_config(server, 33333), "0.0.0.0", "legacy-reality")
            .unwrap();
        assert_eq!(imported.listen_address, "0.0.0.0");
        assert_eq!(imported.source_inbound_tag.as_deref(), Some("legacy-reality"));
        assert_eq!(imported.deploy_status, "deploying");
        let serialized = serde_json::to_string(&imported).unwrap();
        assert!(!serialized.contains("private-secret"));
        assert!(!serialized.contains("source_inbound_tag"));
        assert!(!serialized.contains("legacy-reality"));

        assert!(db
            .create_imported_proxy_node(&proxy_node_config(server, 33334), "::", "legacy-reality")
            .is_err());
        db.clear_proxy_node_source_tags(server).unwrap();
        assert_eq!(db.proxy_node(imported.id).unwrap().unwrap().source_inbound_tag, None);
    }

    #[test]
    fn automatic_proxy_ports_are_unique_per_server_and_reused_across_servers() {
        let db = db();
        let server = db
            .create_node(&Node { name: "server-a".into(), ..Default::default() }, "server-a-token")
            .unwrap();
        let other_server = db
            .create_node(&Node { name: "server-b".into(), ..Default::default() }, "server-b-token")
            .unwrap();
        let config = proxy_node_config(server, 1);

        let first = db.create_proxy_node_auto_port(&config, AUTO_PROXY_PORT_MIN).unwrap();
        let second = db.create_proxy_node_auto_port(&config, AUTO_PROXY_PORT_MIN).unwrap();
        let on_other_server =
            db.create_proxy_node_auto_port(&proxy_node_config(other_server, 1), AUTO_PROXY_PORT_MIN).unwrap();

        assert_eq!(first.listen_port, AUTO_PROXY_PORT_MIN);
        assert_eq!(second.listen_port, AUTO_PROXY_PORT_MIN + 1);
        assert_eq!(on_other_server.listen_port, AUTO_PROXY_PORT_MIN);
    }

    /// PRAGMA settings are per connection, so a value read through any other
    /// handle proves nothing about the one the hub writes through.
    #[test]
    fn the_tuning_pragmas_reach_the_connection_the_hub_uses() {
        let db = db();
        let conn = db.conn();
        let read = |p: &str| conn.query_row(&format!("PRAGMA {p}"), [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(read("cache_size"), -8192, "8 MiB of page cache");
        assert_eq!(read("wal_autocheckpoint"), 256);
        assert_eq!(read("journal_size_limit"), 1_048_576);
        assert_eq!(read("busy_timeout"), 5_000);
    }

    /// A real file, since these three tests exist to exercise what happens to
    /// one. Removed by the test that created it.
    struct Scratch(String);

    impl Scratch {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("monitor-test-{}.db", rand::random::<u64>()))
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm", ".copy"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0));
            }
        }
    }

    /// Backup and restore are the two operations that can lose every row in the
    /// database, so this exercises the whole path: take a copy, modify the live
    /// database, restore the copy, and confirm the change is gone.
    #[test]
    fn a_backup_restores_the_database_it_was_taken_from() {
        let scratch = Scratch::new();
        let copy = format!("{}.copy", scratch.0);
        let db = Db::open(&scratch.0).unwrap();
        let kept =
            db.create_node(&Node { name: "backed-up".into(), ..Default::default() }, "token-kept").unwrap();
        db.backup_into(&copy).unwrap();

        // Everything after the copy must disappear on restore, including a node
        // that reclaimed the deleted one's id.
        db.delete_node(kept).unwrap();
        db.create_node(&Node { name: "after".into(), ..Default::default() }, "token-after").unwrap();

        db.check_backup(&copy).unwrap();
        db.restore_from(&copy).unwrap();
        let back = db.nodes().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].name.as_str(), back[0].token.as_str()), ("backed-up", "token-kept"));
        assert!(db.node_by_token("token-after").unwrap().is_none(), "the row made after the copy is gone");

        // The connection remains the hub's: it can write, it is on the schema this
        // build expects, and it retains the journal mode the hub opened with --
        // the copy `VACUUM INTO` wrote is not in WAL mode.
        node(&db, 1);
        let conn = db.conn();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
        drop(conn);
        let _ = std::fs::remove_file(&copy);
    }

    /// The upload behind restore is an externally supplied file. Each case here
    /// is a way for it not to be a hub backup, and every one must be caught
    /// before a single page is copied over live data.
    #[test]
    fn restore_refuses_anything_that_is_not_a_backup_of_this_hub() {
        let scratch = Scratch::new();
        let db = Db::open(&scratch.0).unwrap();
        let bad = format!("{}.copy", scratch.0);
        // Each refusal carries a message the panel shows, never a bare 500.
        let refused = |why: &str| {
            let e = db.check_backup(&bad).unwrap_err();
            assert!(e.downcast_ref::<crate::Shown>().is_some(), "{why}: {e:#}");
            e.to_string()
        };

        std::fs::write(&bad, b"this is not a database at all").unwrap();
        refused("not SQLite");

        let _ = std::fs::remove_file(&bad);
        let empty = Connection::open(&bad).unwrap();
        empty.execute_batch("CREATE TABLE unrelated (a)").unwrap();
        refused("SQLite, but not this schema");

        // A file carrying its own code where a table belongs: the restore copies
        // pages, so that schema would become the one the hub runs every statement
        // against.
        empty.execute_batch(&SCHEMA.replace("PRAGMA journal_mode = WAL;", "")).unwrap();
        empty
            .execute_batch(
                "DROP TABLE session; CREATE VIEW session AS SELECT 1 AS token_hash, 2 AS expires_at",
            )
            .unwrap();
        refused("a view where a table belongs");

        // Tables with the right names and none of the right columns. Every
        // gate above passes: it is a healthy SQLite file, it carries no view or
        // trigger, all ten names are present, it stamps itself with this build's
        // version and uses the same page size. Restoring copies pages, so those
        // columns would become the ones the hub runs every statement against,
        // leaving the panel reporting a failed restore over a database already
        // replaced.
        let _ = std::fs::remove_file(&bad);
        let shaped = Connection::open(&bad).unwrap();
        for table in TABLES {
            shaped.execute_batch(&format!("CREATE TABLE {table} (x TEXT)")).unwrap();
        }
        shaped.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")).unwrap();
        // Which table fails first follows the order of TABLES and is incidental;
        // naming the table and the columns is what matters.
        let missing = refused("tables without their columns");
        assert!(missing.contains("表缺少字段"), "{missing}");

        // From a hub carrying a schema this build has never seen.
        let _ = std::fs::remove_file(&bad);
        let newer = Connection::open(&bad).unwrap();
        newer.execute_batch(SCHEMA).unwrap();
        migrate(&newer, 0).unwrap();
        newer.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1)).unwrap();
        refused("from a newer hub");

        newer.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")).unwrap();
        db.check_backup(&bad).unwrap();
    }

    #[test]
    fn an_older_backup_gains_proxy_tables_before_schema_validation() {
        let scratch = Scratch::new();
        release_file(&scratch.0);
        let db = Db::open(":memory:").unwrap();
        db.check_backup(&scratch.0).unwrap();
        let candidate = Connection::open(&scratch.0).unwrap();
        let version: i64 = candidate.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(
            candidate
                .query_row("SELECT total_rx FROM traffic WHERE node_id=1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            5_000
        );
        for table in ["proxy_instance", "proxy_user", "proxy_node"] {
            let found: i64 = candidate
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table}");
        }
    }

    #[test]
    fn a_v10_backup_gains_proxy_nodes_before_schema_validation() {
        let scratch = Scratch::new();
        {
            let db = Db::open(&scratch.0).unwrap();
            let node_id =
                db.create_node(&Node { name: "v10".into(), ..Default::default() }, "v10-token").unwrap();
            db.observe_singbox(node_id, Some("running"), Some(Some("1.14.0")), None, None, 10).unwrap();
            let conn = db.conn();
            conn.execute("DROP TABLE proxy_node", []).unwrap();
            conn.execute_batch("PRAGMA user_version = 10").unwrap();
        }

        let db = Db::open(":memory:").unwrap();
        db.check_backup(&scratch.0).unwrap();
        let candidate = Connection::open(&scratch.0).unwrap();
        let version: i64 = candidate.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(
            candidate
                .query_row("SELECT status FROM proxy_instance WHERE node_id=1", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "running"
        );
        assert_eq!(
            candidate
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='proxy_node'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_v11_backup_gains_proxy_import_fields_before_schema_validation() {
        let scratch = Scratch::new();
        let copy = format!("{}.copy", scratch.0);
        let db = Db::open(&scratch.0).unwrap();
        let server = db
            .create_node(
                &Node { name: "legacy-import-server".into(), ..Default::default() },
                "legacy-import-token",
            )
            .unwrap();
        let node = db.create_proxy_node(&proxy_node_config(server, 33333)).unwrap();
        db.backup_into(&copy).unwrap();

        let old = Connection::open(&copy).unwrap();
        old.execute_batch(
            "DROP INDEX proxy_node_source_tag;
             ALTER TABLE proxy_node DROP COLUMN source_inbound_tag;
             ALTER TABLE proxy_node DROP COLUMN listen_address;
             PRAGMA user_version = 11;",
        )
        .unwrap();
        drop(old);

        db.check_backup(&copy).unwrap();
        let migrated = Connection::open(&copy).unwrap();
        let version = migrated.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let (listen, source): (String, Option<String>) = migrated
            .query_row(
                "SELECT listen_address, source_inbound_tag FROM proxy_node WHERE id=?1",
                [node.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(listen, DEFAULT_PROXY_LISTEN_ADDRESS);
        assert_eq!(source, None);
    }

    #[test]
    fn a_v12_backup_gains_proxy_user_fields_and_assignment_table_before_validation() {
        let scratch = Scratch::new();
        let copy = format!("{}.copy", scratch.0);
        let db = Db::open(&scratch.0).unwrap();
        let server =
            db.create_node(&Node { name: "v12-server".into(), ..Default::default() }, "v12-token").unwrap();
        let (user, _) = db
            .create_proxy_user_with_nodes("v12-user", "f3a9c7de-bc1a-4239-8c56-05e7a0984e41", true, "", &[])
            .unwrap();
        let user_id = user.id;
        db.conn().execute("UPDATE proxy_user SET quota=4096, expire_at=1234 WHERE id=?1", [user_id]).unwrap();
        db.create_proxy_node(&proxy_node_config(server, 33333)).unwrap();
        db.backup_into(&copy).unwrap();

        let old = Connection::open(&copy).unwrap();
        old.execute_batch(
            "DROP TABLE proxy_user_node;
             ALTER TABLE proxy_user DROP COLUMN updated_at;
             ALTER TABLE proxy_user DROP COLUMN note;
             PRAGMA user_version = 12;",
        )
        .unwrap();
        drop(old);

        db.check_backup(&copy).unwrap();
        let migrated = Connection::open(&copy).unwrap();
        let version = migrated.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let (username, quota, expiry, note, updated_at, created_at): (
            String,
            i64,
            Option<i64>,
            String,
            i64,
            i64,
        ) = migrated
            .query_row(
                "SELECT username, quota, expire_at, note, updated_at, created_at FROM proxy_user WHERE id=?1",
                [user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .unwrap();
        assert_eq!(username, "v12-user");
        assert_eq!(quota, 4096);
        assert_eq!(expiry, Some(1234));
        assert_eq!(note, "");
        assert_eq!(updated_at, created_at);
        assert_eq!(
            migrated
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='proxy_user_node'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    /// `oldest` is what the data page compares against the retention window, so it
    /// must span both pruned tables rather than whichever happens to have rows.
    #[test]
    fn stats_report_the_earliest_history_row_and_the_window_it_is_kept_for() {
        let scratch = Scratch::new();
        let db = Db::open(&scratch.0).unwrap();
        let id = node(&db, 1);
        let now = Utc::now().timestamp();

        assert_eq!(db.stats().unwrap()["oldest"], serde_json::Value::Null, "no history, no start");
        assert_eq!(db.stats().unwrap()["retention"], 7, "an unset window is the default");

        db.insert_metric(id, now - 3 * 86_400, &serde_json::json!({"cpu": 1.0})).unwrap();
        assert_eq!(db.stats().unwrap()["oldest"], now - 3 * 86_400);

        // Older, and in the other table: the earlier of the two prevails. The probe
        // must be assigned, or the result is not this node's to file.
        let task = db
            .save_ping_task(&PingTask {
                id: 0,
                name: "p".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes: vec![id],
                ..Default::default()
            })
            .unwrap();
        db.insert_pings(id, &[(task, now - 9 * 86_400, 12)]).unwrap();
        assert_eq!(db.stats().unwrap()["oldest"], now - 9 * 86_400);

        db.set("retention_days", "9999").unwrap();
        assert_eq!(db.stats().unwrap()["retention"], 3_650, "a stored window is still clamped");
    }

    /// Deleted rows leave free pages behind; only a rebuild returns them to the
    /// filesystem, and in WAL mode only after the checkpoint.
    #[test]
    fn vacuum_gives_the_deleted_pages_back_to_the_filesystem() {
        let scratch = Scratch::new();
        let db = Db::open(&scratch.0).unwrap();
        let id = node(&db, 1);
        let now = Utc::now().timestamp();
        let sample = serde_json::json!({"cpu": 1.0, "mem_used": 1, "swap_used": 1, "disk_used": 1,
            "net_rx": 1, "net_tx": 1, "tcp": 1, "udp": 1, "procs": 1});
        // Every row strictly before `now`: `prune(0)` cuts at its own `Utc::now()`,
        // and `ts < cutoff` would spare a row stamped in the same second the prune
        // runs.
        for i in 1..=20_000 {
            db.insert_metric(id, now - i, &sample).unwrap();
        }
        let _ = db.conn().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        let fat = on_disk(&scratch.0);
        db.prune(0).unwrap();

        let freed = db.vacuum().unwrap();
        assert!(freed > 0, "a vacuum after deleting 20 000 rows has to return space");
        assert!(on_disk(&scratch.0) < fat);
        assert_eq!(db.stats().unwrap()["rows"]["metric"], 0);
        assert_eq!(db.nodes().unwrap().len(), 1, "vacuum keeps the rows that are left");
    }

    fn node(db: &Db, reset_day: u32) -> i64 {
        let token = format!("token-{}", rand::random::<u32>());
        db.create_node(&Node { name: "n".into(), traffic_reset_day: reset_day, ..Default::default() }, &token)
            .unwrap()
    }

    /// The country is derived from the address, so it must be dropped the moment
    /// the address no longer matches -- and only then, or every reconnect would
    /// spend an outbound request repeating a settled lookup.
    #[test]
    fn a_country_outlives_a_reconnect_and_dies_with_the_address_it_came_from() {
        let db = db();
        let id = node(&db, 1);
        let facts = serde_json::json!({"hostname": "h"});
        let save = |ip: &str| db.save_facts(id, &facts, ip, ip).unwrap();
        let stored = || db.node(id).unwrap().unwrap().country;

        assert!(save("198.51.100.4"), "a node with no country is owed a lookup");
        db.set_country(id, "US", "198.51.100.4").unwrap();
        assert!(!save("198.51.100.4"), "the same address asks nothing a second time");
        assert_eq!(stored(), "US");
        assert!(save("203.0.113.9"), "a new address is a new question");
        assert_eq!(stored(), "", "and the old answer no longer shows");
        assert!(db.country_owed(id, "203.0.113.9").unwrap(), "owed until an answer lands");
        assert!(!db.country_owed(id, "198.51.100.4").unwrap(), "nothing is owed for an address left behind");

        // A lookup issued for the old address, arriving after the move.
        db.set_country(id, "US", "198.51.100.4").unwrap();
        assert_eq!(stored(), "", "an answer about an address the node has left is dropped");
        db.set_country(id, "JP", "203.0.113.9").unwrap();
        assert_eq!(stored(), "JP", "the answer about the address it is at now lands");
        assert!(!db.country_owed(id, "203.0.113.9").unwrap());

        // The source, not the connection address, is what the country belongs to:
        // a proxy exit changing under a node with a public interface address
        // leaves the badge alone.
        assert!(!db.save_facts(id, &facts, "198.51.100.77", "203.0.113.9").unwrap());
        assert_eq!(stored(), "JP");
        // Nothing public to look up: no country, and none owed.
        assert!(!db.save_facts(id, &facts, "192.168.1.2", "").unwrap());
        assert_eq!(stored(), "");
    }

    /// A reboot: the first hello carries only the v6, the next one the v4 again.
    /// The detour spends the node's hourly lookup, so the address returned to
    /// must be answered from the row.
    #[test]
    fn a_country_returns_with_the_address_it_came_from() {
        let db = db();
        let id = node(&db, 1);
        let facts = serde_json::json!({});
        let save = |source: &str| db.save_facts(id, &facts, "198.51.100.4", source).unwrap();
        let stored = || db.node(id).unwrap().unwrap().country;
        let (v4, v6) = ("198.51.100.4", "2001:db8::5");

        save(v4);
        db.set_country(id, "RU", v4).unwrap();
        assert!(save(v6), "an address never answered is asked about");
        db.set_country(id, "US", v6).unwrap();
        assert!(!save(v4), "the address before it is not asked about again");
        assert_eq!(stored(), "RU");
        assert!(!save(v6), "nor, after that, the one in between");
        assert_eq!(stored(), "US");

        // Addresses never answered pass through without displacing the last answer.
        assert!(save("203.0.113.9"));
        assert!(save("203.0.113.10"));
        assert!(!save(v6));
        assert_eq!(stored(), "US");
    }

    /// A hub before schema 5 looked every country up from `ip`. After the
    /// upgrade a node whose source is still `ip` keeps its badge, and one whose
    /// public interface address now takes precedence is asked about again.
    #[test]
    fn countries_stored_before_the_source_column_belong_to_the_connection_address() {
        let db = db();
        let (kept, moved) = (node(&db, 1), node(&db, 1));
        let facts = serde_json::json!({});
        for id in [kept, moved] {
            db.save_facts(id, &facts, "198.51.100.4", "198.51.100.4").unwrap();
            db.set_country(id, "SG", "198.51.100.4").unwrap();
        }
        {
            let conn = db.conn();
            conn.execute_batch("ALTER TABLE node DROP COLUMN country_ip").unwrap();
            migrate(&conn, 4).unwrap();
        }
        assert!(!db.save_facts(kept, &facts, "198.51.100.4", "198.51.100.4").unwrap());
        assert_eq!(db.node(kept).unwrap().unwrap().country, "SG");
        assert!(db.save_facts(moved, &facts, "198.51.100.4", "2001:db8::5").unwrap());
        assert_eq!(db.node(moved).unwrap().unwrap().country, "");
    }

    #[test]
    fn traffic_survives_a_reboot_instead_of_resetting() {
        let db = db();
        let id = node(&db, 1);

        // The first report only establishes the baseline.
        let t = db.accumulate(id, "boot-a", (5_000, 3_000), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (0, 0));

        let t = db.accumulate(id, "boot-a", (9_000, 6_000), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (4_000, 3_000));

        // Reboot: a new boot_id with counters restarting near zero. The total must
        // not fall back to the fresh value, and the 700 bytes moved before the
        // first report are not booked, nothing having measured them.
        let t = db.accumulate(id, "boot-b", (700, 400), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (4_000, 3_000), "a reboot must not reset the total");

        // Counting resumes from the new baseline.
        let t = db.accumulate(id, "boot-b", (1_700, 900), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (5_000, 3_500));
        assert_eq!((t.month_rx, t.month_tx), (5_000, 3_500));
    }

    /// One install command pasted onto a second machine: both agents answer for
    /// the same node and evict each other, so the hub sees two boot_ids
    /// alternating, each with its own lifetime counter. Booking those would add
    /// roughly 180 GB per swap to a total that only increases.
    #[test]
    fn two_machines_sharing_one_token_cannot_inflate_the_total() {
        let db = db();
        let id = node(&db, 1);
        let (a, b) = (100_000_000_000, 80_000_000_000); // two lifetime counters

        db.accumulate(id, "boot-a", (a, a), Local::now()).unwrap();
        let t = db.accumulate(id, "boot-a", (a + 1_000, a + 1_000), Local::now()).unwrap();
        assert_eq!(t.total_rx, 1_000, "the real machine's own traffic still counts");

        // Every swap presents a boot_id with no baseline, so every swap books
        // nothing.
        for round in 0..3 {
            db.accumulate(id, "boot-b", (b + round, b + round), Local::now()).unwrap();
            db.accumulate(id, "boot-a", (a + 1_000 + round, a + 1_000 + round), Local::now()).unwrap();
        }
        let t = db.all_traffic()[&id].clone();
        assert!(t.total_rx < 10_000, "six swaps booked {} bytes, not a lifetime counter", t.total_rx);
    }

    #[test]
    fn a_shrinking_reading_re_aligns_instead_of_re_counting_history() {
        let db = db();
        let id = node(&db, 1);
        db.accumulate(id, "boot-a", (10_000, 10_000), Local::now()).unwrap();
        let t = db.accumulate(id, "boot-a", (12_000, 12_000), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (2_000, 2_000));

        // The same boot with a reduced reading: an interface included in the sum
        // has gone, so this is the remainder of the machine's history rather than
        // new bytes.
        let t = db.accumulate(id, "boot-a", (500, 500), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (2_000, 2_000));

        // Aligned to the smaller baseline, counting resumes from there.
        let t = db.accumulate(id, "boot-a", (900, 900), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (2_400, 2_400));

        // A new boot realigns identically, for the same reason: it has no baseline
        // either.
        let t = db.accumulate(id, "boot-b", (300, 300), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (2_400, 2_400));

        // One direction shrinking does not deprive the other of its increment.
        let t = db.accumulate(id, "boot-b", (100, 900), Local::now()).unwrap();
        assert_eq!((t.total_rx, t.total_tx), (2_400, 3_000));
    }

    /// The two counters that restart on their own schedules, against a total that
    /// never does. Each derives from its own stored date, so a rollover must leave
    /// the other untouched.
    #[test]
    fn day_and_month_restart_independently_while_the_total_keeps_climbing() {
        let db = db();
        let id = node(&db, 1);
        db.accumulate(id, "boot-a", (0, 0), Local::now()).unwrap();
        let t = db.accumulate(id, "boot-a", (8_000, 4_000), Local::now()).unwrap();
        assert_eq!((t.day_rx, t.day_tx), (8_000, 4_000));
        assert_eq!((t.month_rx, t.month_tx), (8_000, 4_000));

        // Midnight passes, forced through the stored date the rollover reads.
        db.conn().execute("UPDATE traffic SET day_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();
        let t = db.accumulate(id, "boot-a", (9_500, 4_600), Local::now()).unwrap();
        assert_eq!((t.day_rx, t.day_tx), (1_500, 600), "a new day counts only this report's delta");
        assert_eq!(t.month_rx, 9_500, "the month is not a day");
        assert_eq!(t.total_rx, 9_500, "and the total is neither");

        // The billing period then rolls over, partway through that same day.
        db.conn().execute("UPDATE traffic SET month_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();
        let t = db.accumulate(id, "boot-a", (10_000, 4_700), Local::now()).unwrap();
        assert_eq!((t.month_rx, t.month_tx), (500, 100), "a new period counts only this report's delta");
        assert_eq!((t.day_rx, t.day_tx), (2_000, 700), "the day carries on across a billing rollover");
        assert_eq!(t.total_rx, 10_000, "lifetime total is untouched by either rollover");
    }

    /// A reading is dated by its arrival, which for one held back over a boundary
    /// is earlier than its booking. It must never take the row back into a period
    /// already stamped on it, as the panel stamps the current one with a
    /// correction. A later date still moves the period, as a changed reset day
    /// requires, and a stamp later than today is not held to.
    #[test]
    fn a_reading_is_never_booked_into_a_period_already_left() {
        use chrono::TimeZone;
        let db = db();
        let id = node(&db, 20);
        let on = |d: u32| Local.with_ymd_and_hms(2026, 9, d, 12, 0, 0).unwrap();
        db.accumulate(id, "boot-a", (1_000, 0), on(24)).unwrap();
        db.accumulate(id, "boot-a", (3_000, 0), on(24)).unwrap();
        let t = db.accumulate(id, "boot-a", (3_500, 0), on(23)).unwrap();
        assert_eq!(t.day_rx, 2_500, "a reading dated the day before does not restart today");
        assert_eq!(t.month_start, "2026-09-20");

        // The reset day moves to the 1st: the period now starts earlier, and a
        // reading dated after the stamp still switches to it.
        db.update_node(id, &NodePatch { traffic_reset_day: Some(1), ..Default::default() }).unwrap();
        let t = db.accumulate(id, "boot-a", (4_000, 0), on(24)).unwrap();
        assert_eq!((t.month_start.as_str(), t.month_rx), ("2026-09-01", 500));

        // A correction stamps the current period; a reading from the one before,
        // held over the boundary, lands on top of it rather than discarding it.
        db.set_traffic(id, &TrafficPatch { month_rx: Some(10_000), ..Default::default() }).unwrap();
        let t = db.accumulate(id, "boot-a", (4_500, 0), Local::now() - chrono::Duration::days(40)).unwrap();
        assert_eq!(t.month_rx, 10_500, "the correction survives a reading dated before it");

        // A clock a year ahead stamps its own day and period. Once it is stepped
        // back, the next reading returns the row to today.
        db.accumulate(id, "boot-a", (5_000, 0), Local::now() + chrono::Duration::days(365)).unwrap();
        db.accumulate(id, "boot-a", (5_200, 0), Local::now()).unwrap();
        let t = db.all_traffic().remove(&id).unwrap();
        assert_eq!((t.day_rx, t.month_rx), (200, 200), "today reads what moved today, not zero");
    }

    /// The other half of the rollover: the counters restart on the node's next
    /// report, so a node silent since before a boundary still holds the previous
    /// period's bytes on disk. The read side must not return those.
    #[test]
    fn a_node_that_went_quiet_before_a_boundary_reads_as_zero_this_period() {
        let db = db();
        let id = node(&db, 1);
        db.accumulate(id, "boot-a", (0, 0), Local::now()).unwrap();
        db.accumulate(id, "boot-a", (8_000, 4_000), Local::now()).unwrap();
        assert_eq!(db.all_traffic()[&id].day_rx, 8_000, "still today, so it still counts");

        // Offline across both boundaries, with no report to restart either.
        db.conn()
            .execute(
                "UPDATE traffic SET day_start='1999-01-01', month_start='1999-01-01' WHERE node_id=?1",
                [id],
            )
            .unwrap();
        let t = db.all_traffic()[&id].clone();
        assert_eq!((t.day_rx, t.day_tx), (0, 0), "yesterday's bytes are not today's");
        assert_eq!((t.month_rx, t.month_tx), (0, 0), "last period's bytes are not this period's");
        assert_eq!(t.month_start, period_start(Local::now().date_naive(), 1).to_string());
        assert_eq!((t.total_rx, t.total_tx), (8_000, 4_000), "the lifetime total never resets");
    }

    /// Received 3, sent 5. "up" is the node's upload, which it sends.
    #[test]
    fn usage_is_counted_the_way_the_plan_meters_it() {
        let t = Traffic { month_rx: 3, month_tx: 5, ..Default::default() };
        assert_eq!(["sum", "up", "down", "max"].map(|mode| t.month_used(mode)), [8, 5, 3, 5]);
    }

    #[test]
    fn period_start_handles_short_months_and_wraparound() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        // Reset on the 15th, today the 20th: the current month.
        assert_eq!(period_start(d(2026, 3, 20), 15), d(2026, 3, 15));
        // The reset day itself counts as the start of the new period.
        assert_eq!(period_start(d(2026, 3, 15), 15), d(2026, 3, 15));
        // Before the reset day the period began in the previous month.
        assert_eq!(period_start(d(2026, 3, 10), 15), d(2026, 2, 15));
        // January rolls back into the previous year.
        assert_eq!(period_start(d(2026, 1, 10), 15), d(2025, 12, 15));
        // Day 31 in February clamps to the 28th; 2028 is a leap year.
        assert_eq!(period_start(d(2026, 2, 28), 31), d(2026, 2, 28));
        assert_eq!(period_start(d(2028, 2, 29), 31), d(2028, 2, 29));
    }

    #[test]
    fn deleting_a_node_takes_its_data_with_it() {
        let db = db();
        let id = node(&db, 1);
        let probe = |nodes| PingTask {
            id: 0,
            name: "cm".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes,
            ..Default::default()
        };
        let task = db.save_ping_task(&probe(vec![id])).unwrap();
        db.accumulate(id, "b", (10, 10), Local::now()).unwrap();
        db.insert_metric(id, 1, &serde_json::json!({"cpu": 1.0})).unwrap();
        db.insert_pings(id, &[(task, 1, 42)]).unwrap();
        db.delete_node(id).unwrap();
        assert!(db.node(id).unwrap().is_none());
        assert_eq!(db.metrics(id, 0, 60).unwrap().len(), 0);
        assert!(!db.all_traffic().contains_key(&id));
        // Ticked in an editor opened before the delete: named, not a 500.
        let gone = db.save_ping_task(&probe(vec![id])).unwrap_err();
        let expected = format!("服务器 {id} 不存在，可能已被删除");
        assert_eq!(
            gone.downcast_ref::<crate::Shown>().map(|s| s.0.as_str()),
            Some(expected.as_str()),
            "{gone:#}"
        );

        // `ping_record` has no foreign key to cascade through, and SQLite reassigns
        // the deleted id to the next node created: without the sweep in
        // `delete_node` the new machine would draw the old one's chart.
        let fresh = node(&db, 1);
        assert_eq!(fresh, id, "the id is reused, which is what makes this reachable");
        db.save_ping_task(&PingTask { id: task, nodes: vec![fresh], ..probe(vec![]) }).unwrap();
        assert!(db.ping_records(fresh, 0, 60).unwrap().0.is_empty(), "and it starts with no history");
    }

    /// The mirror of the sweep above, on the other key of the same table. SQLite
    /// reuses a deleted probe's id as well, and the chart selects on a node's
    /// assignments, so the removed probe's samples would reappear under the new
    /// probe's name with its timeouts folded into the new loss figure.
    #[test]
    fn deleting_a_probe_takes_its_history_with_it() {
        let db = db();
        let id = node(&db, 1);
        let probe = |name: &str| PingTask {
            id: 0,
            name: name.into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![id],
            ..Default::default()
        };
        let old = db.save_ping_task(&probe("tokyo")).unwrap();
        db.insert_pings(id, &[(old, 1, 999)]).unwrap();
        db.delete_ping_task(old).unwrap();

        let fresh = db.save_ping_task(&probe("singapore")).unwrap();
        assert_eq!(fresh, old, "the id is reused, which is what makes this reachable");
        assert!(db.ping_records(id, 0, 60).unwrap().0.is_empty(), "and it starts with no history");
    }

    /// Counted directly from the table rather than read back through
    /// `ping_records`: that query filters on the node's assignments, so a row
    /// written under a probe it does not have is invisible to it. An assertion
    /// made through it therefore could not fail for the write this test exists to
    /// prevent.
    #[test]
    fn a_result_for_a_probe_this_node_does_not_have_is_not_stored() {
        let db = db();
        let mine = node(&db, 1);
        let other = node(&db, 1);
        let rows = || db.conn().query_row("SELECT COUNT(*) FROM ping_record", [], |r| r.get::<_, i64>(0));
        let task = db
            .save_ping_task(&PingTask {
                id: 0,
                name: "p".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes: vec![mine],
                ..Default::default()
            })
            .unwrap();

        db.insert_pings(mine, &[(task, 1, 42)]).unwrap();
        assert_eq!(rows().unwrap(), 1, "the node the probe is assigned to files its own result");

        // A probe that exists but belongs to another node, and ids naming no probe
        // at all: what a node token can place on the wire.
        db.insert_pings(other, &[(task, 1, 42)]).unwrap();
        for invented in [7, 999_999, i64::from(i32::MAX) + 1] {
            db.insert_pings(mine, &[(invented, 1, 42)]).unwrap();
        }
        assert_eq!(rows().unwrap(), 1, "nothing else reaches the table");

        // Deleting the probe also ends its node's results, so one already in flight
        // cannot land after the sweep and be inherited by the next probe to take
        // the id.
        db.delete_ping_task(task).unwrap();
        db.insert_pings(mine, &[(task, 2, 42)]).unwrap();
        assert_eq!(rows().unwrap(), 0, "a late result for a deleted probe is dropped");
    }

    /// The strings in a `hello` come from an unvouched machine, and six of them go
    /// straight into the frame pushed to the public page every two seconds, so
    /// their length cannot be the node's to choose. `api` enforces the same bound
    /// on the one string `agent_register` accepts.
    #[test]
    fn facts_from_an_unvouched_machine_cannot_choose_their_own_length() {
        let db = db();
        let id = node(&db, 1);
        db.save_facts(id, &serde_json::json!({"os": "A".repeat(10_000), "hostname": "x\u{7}y"}), "ip", "")
            .unwrap();
        let stored = db.node(id).unwrap().unwrap();
        assert_eq!(stored.os.chars().count(), 128);
        assert_eq!(stored.hostname, "xy", "control characters break the panel's rows");
    }

    /// A correction must survive the node's return. `all_traffic` gates the month
    /// figures on the period they were written for, and `accumulate` restarts the
    /// counter when the stored period is stale, so a correction left under the
    /// previous period would read as zero and then be discarded.
    #[test]
    fn a_month_correction_is_stamped_with_the_period_it_was_made_in() {
        let db = db();
        let id = node(&db, 1);
        db.accumulate(id, "boot-a", (0, 0), Local::now()).unwrap();
        // A node silent since before its reset day still holds the old period.
        db.conn().execute("UPDATE traffic SET month_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();

        db.set_traffic(
            id,
            &TrafficPatch {
                total_rx: Some(4_000),
                total_tx: Some(2_000),
                month_rx: Some(300),
                month_tx: Some(100),
            },
        )
        .unwrap();
        let t = db.all_traffic().remove(&id).unwrap();
        assert_eq!((t.month_rx, t.month_tx), (300, 100), "the correction reads back as this period's");

        let t = db.accumulate(id, "boot-a", (500, 50), Local::now()).unwrap();
        assert_eq!((t.month_rx, t.month_tx), (800, 150), "and the next report adds to it");
        assert_eq!((t.total_rx, t.total_tx), (4_500, 2_050));
    }

    #[test]
    fn partial_edits_keep_other_settings_and_live_counters() {
        let db = db();
        let id = node(&db, 1);
        let patch = |v| serde_json::from_value::<NodePatch>(v).unwrap();
        db.update_node(
            id,
            &patch(serde_json::json!({"public":false,"remark":"private","expires_at":"2030-01-01"})),
        )
        .unwrap();
        db.update_node(id, &patch(serde_json::json!({"price":20}))).unwrap();
        let n = db.node(id).unwrap().unwrap();
        assert!(!n.public);
        assert_eq!(n.remark, "private");
        assert_eq!(n.expires_at.as_deref(), Some("2030-01-01"));
        db.update_node(id, &patch(serde_json::json!({"price":0,"expires_at":null}))).unwrap();
        let n = db.node(id).unwrap().unwrap();
        assert_eq!(n.price, 0.0);
        assert_eq!(n.expires_at, None);

        db.accumulate(id, "boot", (0, 0), Local::now()).unwrap();
        db.accumulate(id, "boot", (120_000, 10_000), Local::now()).unwrap();
        db.set_traffic(id, &TrafficPatch { month_tx: Some(3_000), ..Default::default() }).unwrap();
        let t = db.all_traffic().remove(&id).unwrap();
        assert_eq!((t.total_rx, t.total_tx, t.month_rx, t.month_tx), (120_000, 10_000, 120_000, 3_000));

        db.update_node(id, &patch(serde_json::json!({"traffic_reset_day":2}))).unwrap();
        db.set_traffic(id, &TrafficPatch { month_rx: Some(7_000), ..Default::default() }).unwrap();
        let t = db.all_traffic().remove(&id).unwrap();
        assert_eq!((t.month_rx, t.month_tx), (7_000, 0));
        // Correcting only a lifetime total cannot revive the previous month's
        // bytes.
        db.conn()
            .execute("UPDATE traffic SET month_start='1999-01-01',month_tx=999 WHERE node_id=?1", [id])
            .unwrap();
        db.set_traffic(id, &TrafficPatch { total_rx: Some(130_000), ..Default::default() }).unwrap();
        assert_eq!(db.all_traffic()[&id].month_tx, 0);
    }

    #[test]
    fn a_token_is_readable_and_rotation_retires_the_old_one() {
        let db = db();
        let id = db.create_node(&Node { name: "n".into(), ..Default::default() }, "first-token").unwrap();

        // Readable, so the panel can display the install command without issuing a
        // new token.
        assert_eq!(db.node(id).unwrap().unwrap().token, "first-token");
        assert_eq!(db.node_by_token("first-token").unwrap(), Some(id));

        db.reset_token(id, "second-token").unwrap();
        assert_eq!(db.node(id).unwrap().unwrap().token, "second-token");
        assert_eq!(db.node_by_token("second-token").unwrap(), Some(id));
        assert_eq!(db.node_by_token("first-token").unwrap(), None, "the old token stops working");
    }

    #[test]
    fn nodes_can_be_reordered_atomically() {
        let db = db();
        let (a, b, c) = (node(&db, 1), node(&db, 1), node(&db, 1));
        let order = || db.nodes().unwrap().iter().map(|n| n.id).collect::<Vec<_>>();
        db.reorder_nodes(&[c, a, b]).unwrap();
        assert_eq!(order(), vec![c, a, b]);

        // Every rejected input leaves the existing order intact. The partial list
        // matters most: a stale tab would otherwise renumber around a node it never
        // saw.
        assert!(db.reorder_nodes(&[a, a, c]).is_err(), "duplicates");
        assert!(db.reorder_nodes(&[a, b]).is_err(), "a node left out");
        assert!(db.reorder_nodes(&[a, b, 9999]).is_err(), "an id that is not a node");
        assert_eq!(order(), vec![c, a, b]);
        // A node added afterwards goes to the end rather than wherever sort 0
        // places it.
        let d = node(&db, 1);
        assert_eq!(db.nodes().unwrap().iter().map(|n| n.id).collect::<Vec<_>>(), vec![c, a, b, d]);
    }

    /// Themes draw probes in the order they first appear in the rows, so the rows
    /// follow the panel's order even when a later probe alone answered in the
    /// window's first bucket.
    #[test]
    fn a_probe_chart_follows_the_panel_order() {
        let db = db();
        let id = node(&db, 1);
        let probe = |name: &str| {
            db.save_ping_task(&PingTask {
                name: name.into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes: vec![id],
                ..Default::default()
            })
            .unwrap()
        };
        let (a, b) = (probe("a"), probe("b"));
        db.reorder_ping_tasks(&[b, a]).unwrap();
        let c = probe("c");
        let listed: Vec<_> = db.ping_tasks().unwrap().iter().map(|t| t.id).collect();
        assert_eq!(listed, vec![b, a, c], "a new probe starts at the end");

        db.insert_pings(id, &[(a, 0, 10), (a, 60, 10), (b, 60, 20), (c, 60, 30)]).unwrap();
        let rows = db.ping_records(id, 0, 60).unwrap().0;
        let drawn: Vec<_> =
            rows.iter().map(|r| (r["task_id"].as_i64().unwrap(), r["ts"].as_i64().unwrap())).collect();
        assert_eq!(drawn, vec![(b, 60), (a, 0), (a, 60), (c, 60)]);
    }

    #[test]
    fn prune_drops_history_but_never_traffic_totals() {
        let db = db();
        let id = node(&db, 1);
        db.accumulate(id, "b", (100, 100), Local::now()).unwrap();
        db.accumulate(id, "b", (900, 900), Local::now()).unwrap();
        let old = Utc::now().timestamp() - 40 * 86_400;
        db.insert_metric(id, old, &serde_json::json!({"cpu": 1.0})).unwrap();
        db.insert_metric(id, Utc::now().timestamp(), &serde_json::json!({"cpu": 2.0})).unwrap();

        db.prune(30).unwrap();
        assert_eq!(db.metrics(id, 0, 60).unwrap().len(), 1);
        assert_eq!(db.all_traffic()[&id].total_rx, 800);
    }

    /// The rekeying in `open()`: rows must survive it, and the chart's query must
    /// emerge able to seek. A migration that leaves every row on the old key fails
    /// silently, and stays silent while the query it exists for scans a node's
    /// entire history.
    #[test]
    fn rekeying_ping_record_keeps_the_rows_and_lets_the_chart_query_seek() {
        let file = std::env::temp_dir().join(format!("monitor-rekey-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_str().unwrap();

        // A database as an older hub left it.
        let old = Connection::open(path).unwrap();
        old.execute_batch(
            "CREATE TABLE ping_record (
               node_id INTEGER NOT NULL, task_id INTEGER NOT NULL,
               ts INTEGER NOT NULL, latency INTEGER NOT NULL,
               PRIMARY KEY (node_id, task_id, ts)
             ) WITHOUT ROWID;
             INSERT INTO ping_record VALUES (1,7,100,12),(1,8,100,34),(1,7,200,56),(2,7,100,78);",
        )
        .unwrap();
        drop(old);

        let db = Db::open(path).unwrap();
        let conn = db.conn();
        let rows: Vec<(i64, i64, i64, i64)> = conn
            .prepare("SELECT node_id, task_id, ts, latency FROM ping_record ORDER BY node_id, ts, task_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows, vec![(1, 7, 100, 12), (1, 8, 100, 34), (1, 7, 200, 56), (2, 7, 100, 78)]);

        // Without the timestamp second in the key the plan stops at `node_id=?`
        // and scans everything beneath it, and the fold in `ping_records` requires
        // rows in time order, which only the seek provides without a sorter.
        let plan: String = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {PING_ROWS}"))
            .unwrap()
            .query_map(params![1, 0, 60], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" | ");
        assert!(plan.contains("node_id=? AND ts>?"), "the window has to be a seek, not a scan: {plan}");
        assert!(!plan.contains("ORDER BY"), "the time order has to come off the key, not a sorter: {plan}");

        // Opening again must not rebuild a table that is already correct.
        drop(conn);
        drop(db);
        assert!(Db::open(path).is_ok());
        let _ = std::fs::remove_file(&file);
    }

    /// Every row of every table, comparable across two opens of one file.
    fn dump(conn: &Connection) -> Vec<String> {
        let mut rows = Vec::new();
        for table in TABLES {
            if columns_of(conn, table).unwrap().is_empty() {
                continue;
            }
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let width = stmt.column_count();
            let read =
                |r: &rusqlite::Row| (0..width).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect();
            let mut found: Vec<String> = stmt
                .query_map([], |r| read(r))
                .unwrap()
                .map(|row: rusqlite::Result<Vec<_>>| format!("{table} {:?}", row.unwrap()))
                .collect();
            found.sort();
            rows.extend(found);
        }
        rows
    }

    /// A file as the oldest release left it, one row in every table.
    fn release_file(path: &str) {
        let old = Connection::open(path).unwrap();
        old.execute_batch(include_str!("testdata/schema-v1.0.0.sql")).unwrap();
        old.execute_batch(
            "INSERT INTO setting VALUES ('site', 'https://hub.example.com');
             INSERT INTO node (id, name, token, ip, country, created_at)
               VALUES (1, 'n', 't', '198.51.100.4', 'US', 1);
             INSERT INTO traffic (node_id, total_rx) VALUES (1, 5000);
             INSERT INTO metric VALUES (1, 60, 12.5, 100, 0, 0, 0, 0, 0, 0, 0);
             INSERT INTO ping_task (id, name, target) VALUES (1, 'cm', '1.1.1.1:443');
             INSERT INTO ping_node VALUES (1, 1);
             INSERT INTO ping_record VALUES (1, 1, 60, 42);
             INSERT INTO session VALUES ('h', 9999999999);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }

    /// v1.0.0's schema, opened by this build through the startup path. Fails on
    /// a column `SCHEMA` gained without a migration or the reverse, on an index
    /// in `SCHEMA` over a column only a migration adds, and on a migration that
    /// changes the data when it runs a second time.
    #[test]
    fn an_upgraded_release_matches_a_fresh_database() {
        let scratch = Scratch::new();
        release_file(&scratch.0);
        let db = Db::open(&scratch.0).unwrap();
        let fresh = Db::open(":memory:").unwrap();
        for table in TABLES {
            assert_eq!(
                columns_of(&db.conn(), table).unwrap(),
                columns_of(&fresh.conn(), table).unwrap(),
                "{table}"
            );
        }
        let upgraded = dump(&db.conn());
        assert_eq!(upgraded.len(), LEGACY_TABLES.len(), "every existing row survives: {upgraded:#?}");

        // An earlier build opening the file stamps its own version, so the next
        // upgrade runs every migration again.
        db.conn().execute_batch("PRAGMA user_version = 3").unwrap();
        drop(db);
        let db = Db::open(&scratch.0).unwrap();
        assert_eq!(dump(&db.conn()), upgraded, "a second run changes nothing");
        let version: i64 = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// The trigger fails the UPDATE in `migrate_to_5` after `migrate_to_4` has
    /// added its columns: a failure part-way through an upgrade.
    #[test]
    fn a_failed_upgrade_leaves_the_file_as_it_was() {
        let scratch = Scratch::new();
        release_file(&scratch.0);
        let state = |c: &Connection| {
            let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            (version, columns_of(c, "node").unwrap(), dump(c))
        };
        let before = {
            let c = Connection::open(&scratch.0).unwrap();
            c.execute_batch(
                "CREATE TRIGGER fail BEFORE UPDATE ON node BEGIN SELECT RAISE(ABORT, 'disk full'); END",
            )
            .unwrap();
            state(&c)
        };
        assert!(Db::open(&scratch.0).is_err());
        assert_eq!(state(&Connection::open(&scratch.0).unwrap()), before, "no step of the upgrade remains");
    }

    /// Dropping `metric.load1` under a database in service. The column is
    /// `NOT NULL` with no default, so a migration that silently failed to run
    /// would not merely leave a stale column: it would prevent every history row
    /// from being written.
    #[test]
    fn dropping_load1_keeps_the_history_and_lets_new_rows_in() {
        let file = std::env::temp_dir().join(format!("monitor-load1-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.to_str().unwrap();

        // A database as a hub predating this build left it: one metric row
        // carrying a load average, stamped with the schema version of the time.
        let old = Connection::open(path).unwrap();
        old.execute_batch(
            "CREATE TABLE metric (
               node_id INTEGER NOT NULL, ts INTEGER NOT NULL,
               cpu REAL NOT NULL, load1 REAL NOT NULL,
               mem_used INTEGER NOT NULL, swap_used INTEGER NOT NULL, disk_used INTEGER NOT NULL,
               net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL,
               tcp INTEGER NOT NULL, udp INTEGER NOT NULL, procs INTEGER NOT NULL,
               PRIMARY KEY (node_id, ts)
             ) WITHOUT ROWID;
             INSERT INTO metric VALUES (1,60,12.5,0.75,100,0,0,0,0,0,0,0);
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(old);

        let db = Db::open(path).unwrap();
        assert!(!schema_mentions(&db.conn(), "metric", "load1").unwrap(), "the column has to be gone");
        // The row remains, along with everything else it carried.
        let kept = &db.metrics(1, 0, 60).unwrap()[0];
        assert_eq!((kept["ts"].as_i64(), kept["cpu"].as_f64()), (Some(60), Some(12.5)));
        // The shape this build inserts now fits the table.
        db.insert_metric(1, 120, &serde_json::json!({"cpu": 2.0, "load": [0.5, 0.4, 0.3]})).unwrap();
        assert_eq!(db.metrics(1, 0, 60).unwrap().len(), 2);

        // Opening again must not attempt to drop a column already removed.
        drop(db);
        assert!(Db::open(path).is_ok());
        let _ = std::fs::remove_file(&file);
    }

    /// Removing a node from a probe must remove the probe from that node's chart.
    /// `ping_record` carries no foreign key to the assignment that produced it, so
    /// the rows outlive it until retention; the window query is what must stop
    /// drawing them, and immediately rather than at the next hourly sweep.
    #[test]
    fn a_probe_taken_off_a_node_stops_appearing_in_its_history() {
        let db = db();
        let id = node(&db, 1);
        let probe = |nodes: Vec<i64>, task| {
            db.save_ping_task(&PingTask {
                id: task,
                name: "cm".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes,
                ..Default::default()
            })
            .unwrap()
        };
        let task = probe(vec![id], 0);
        db.insert_pings(id, &[(task, 100, 42)]).unwrap();
        assert_eq!(db.ping_records(id, 0, 60).unwrap().0.len(), 1, "an assigned probe draws");

        probe(vec![], task);
        assert!(db.ping_records(id, 0, 60).unwrap().0.is_empty(), "an unassigned one does not");

        // The rows remain: reassigning restores the history rather than starting
        // over.
        probe(vec![id], task);
        assert_eq!(db.ping_records(id, 0, 60).unwrap().0.len(), 1, "and it comes back with its history");

        // The names accompany those samples and follow the same filter: a probe
        // name is operator-supplied text that routinely carries a hostname or a
        // customer.
        assert_eq!(db.ping_task_names(id).unwrap()[&task.to_string()], "cm");
        let other = node(&db, 1);
        assert!(
            db.ping_task_names(other).unwrap().as_object().is_some_and(|m| m.is_empty()),
            "a node the probe was never assigned to must not learn its name"
        );
    }

    #[test]
    fn ping_tasks_round_trip_with_their_node_assignments() {
        let db = db();
        let (a, b) = (node(&db, 1), node(&db, 1));
        let id = db
            .save_ping_task(&PingTask {
                id: 0,
                name: "cf".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes: vec![a, b],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(db.ping_tasks_for(a).unwrap().len(), 1);

        // Reassigning to one node must drop the other's copy.
        db.save_ping_task(&PingTask {
            id,
            name: "cf".into(),
            target: "1.1.1.1:443".into(),
            interval: 30,
            nodes: vec![a],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(db.ping_tasks_for(b).unwrap().len(), 0);
        assert_eq!(db.ping_tasks().unwrap()[0].interval, 30);
    }

    #[test]
    fn an_auto_joining_probe_is_assigned_to_nodes_created_after_it() {
        let db = db();
        let existing = node(&db, 1);
        let probe = |auto_join| PingTask {
            id: 0,
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![],
            auto_join,
            ..Default::default()
        };
        let joining = db.save_ping_task(&probe(true)).unwrap();
        db.save_ping_task(&probe(false)).unwrap();
        assert!(db.ping_tasks_for(existing).unwrap().is_empty(), "existing nodes follow the list alone");

        let added = node(&db, 1);
        let assigned: Vec<i64> =
            db.ping_tasks_for(added).unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
        assert_eq!(assigned, [joining]);
        assert!(db.ping_tasks().unwrap()[0].auto_join);

        // A new node takes every auto-joining probe at once, so their count is
        // held to what one agent runs.
        for _ in 1..Db::MAX_PROBES_PER_NODE {
            db.save_ping_task(&probe(true)).unwrap();
        }
        assert!(db.save_ping_task(&probe(true)).is_err());
        assert_eq!(db.ping_tasks().unwrap().len() as i64, Db::MAX_PROBES_PER_NODE + 1);
    }

    /// The editor's list is a snapshot, while `create_node` assigns auto-joining
    /// probes whenever a node registers.
    #[test]
    fn an_edit_applies_only_the_assignments_it_changed() {
        let db = db();
        let (a, b) = (node(&db, 1), node(&db, 1));
        let save = |id, nodes: Vec<i64>, base: Vec<i64>| {
            db.save_ping_task(&PingTask {
                id,
                name: "p".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes,
                auto_join: true,
                base: Some(base),
            })
        };
        let assigned = |id| {
            let mut nodes = db.ping_tasks().unwrap().into_iter().find(|t| t.id == id).unwrap().nodes;
            nodes.sort_unstable();
            nodes
        };
        let id = save(0, vec![a], vec![]).unwrap();
        let joined = node(&db, 1);

        // Opened before `joined` existed, so it is in neither list and stays.
        save(id, vec![a, b], vec![a]).unwrap();
        assert_eq!(assigned(id), [a, b, joined]);
        save(id, vec![b], vec![a, b]).unwrap();
        assert_eq!(assigned(id), [b, joined]);

        // Ticked after joining on its own: already assigned, not an error.
        save(id, vec![b, joined], vec![b]).unwrap();
        assert_eq!(assigned(id), [b, joined]);
        // OR IGNORE leaves the foreign key in force.
        assert!(save(id, vec![b, joined, 9999], vec![b, joined]).is_err(), "an id that is not a node");
        assert_eq!(assigned(id), [b, joined]);

        // A node deleted since the editor opened is in both lists and untouched.
        db.delete_node(b).unwrap();
        save(id, vec![b, joined], vec![b, joined]).unwrap();

        db.delete_ping_task(id).unwrap();
        assert!(save(id, vec![], vec![]).is_err(), "a deleted probe is not reported as saved");
        assert!(db.ping_tasks().unwrap().is_empty());
    }

    /// The agent caps the probe list it will run and drops the remainder with
    /// nothing but a line in its own journal. The hub knows the total, so the hub
    /// issues the refusal; otherwise the panel lists probes that never ran and
    /// charts that stay empty, with the only record on the node.
    #[test]
    fn a_node_cannot_be_given_more_probes_than_the_agent_will_run() {
        let db = db();
        let id = node(&db, 1);
        let save = |task: i64, nodes: Vec<i64>| {
            db.save_ping_task(&PingTask {
                id: task,
                name: "p".into(),
                target: "1.1.1.1:443".into(),
                interval: 60,
                nodes,
                ..Default::default()
            })
        };
        for _ in 0..Db::MAX_PROBES_PER_NODE {
            save(0, vec![id]).unwrap();
        }
        assert_eq!(db.ping_tasks_for(id).unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);

        let refused = save(0, vec![id]).expect_err("one past the cap must be refused");
        assert!(refused.to_string().contains("探测任务"), "{refused}");
        // Rolled back entirely: the probe must not survive its assignment being
        // rejected, or the panel accumulates one that never runs.
        assert_eq!(db.ping_tasks().unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);
        assert_eq!(db.ping_tasks_for(id).unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);

        // Editing an existing probe does not count as adding one.
        let first = db.ping_tasks().unwrap()[0].id;
        save(first, vec![id]).expect("an existing probe can still be edited at the cap");
    }
}
