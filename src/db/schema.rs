use super::*;

pub(super) const SCHEMA: &str = r#"
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
  -- Where the hub saw the node in the address family `ip` is not in, from the
  -- short connection an agent makes over it (`agent_ws::echo`).
  -- Empty until one has arrived, and left as it was when none does: the family
  -- it belongs to must still be held on an interface for it to be shown.
  ip_other TEXT NOT NULL DEFAULT '',
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
  -- Set in the panel and shown on the status page, unlike `remark`. Empty is
  -- none.
  public_remark TEXT NOT NULL DEFAULT '',
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
  net_rx_max INTEGER NOT NULL DEFAULT 0, net_tx_max INTEGER NOT NULL DEFAULT 0,
  cpu_max REAL NOT NULL DEFAULT 0,
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
-- further, then scans every record it ever kept -- see migrate_to_1.
CREATE TABLE IF NOT EXISTS ping_record (
  node_id INTEGER NOT NULL, task_id INTEGER NOT NULL,
  ts INTEGER NOT NULL, latency INTEGER NOT NULL,
  PRIMARY KEY (node_id, ts, task_id)
) WITHOUT ROWID;

-- The hourly tier: charts wider than DETAIL_DAYS read these, and they outlive
-- the minute rows they are folded from. `ts` is the start of the hour, and a
-- row covers the minute rows stamped within it. Keyed like the tables above.
--
-- `minutes` counts the rows folded in and weights each average when buckets
-- spanning several hours merge them. Every column of `metric` is kept, whether
-- or not the chart returns it yet: a minute row is gone after DETAIL_DAYS, and
-- a column added here later would start empty.
CREATE TABLE IF NOT EXISTS metric_hour (
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  ts      INTEGER NOT NULL,
  minutes INTEGER NOT NULL,
  cpu REAL NOT NULL, cpu_max REAL NOT NULL,
  mem_used INTEGER NOT NULL, swap_used INTEGER NOT NULL, disk_used INTEGER NOT NULL,
  net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL,
  net_rx_max INTEGER NOT NULL, net_tx_max INTEGER NOT NULL,
  tcp INTEGER NOT NULL, udp INTEGER NOT NULL, procs INTEGER NOT NULL,
  PRIMARY KEY (node_id, ts)
) WITHOUT ROWID;

-- `latency` is the median of the hour's answers and NULL when none arrived;
-- `lo` and `hi` bound them. `answered` weights the median when buckets merge.
--
-- Foreign keys, unlike `ping_record`: an earlier build run against this file
-- deletes nodes and probes knowing nothing of this table, and SQLite gives a
-- deleted id to the next one created, which would draw the deleted one's
-- hourly latency. The cascade clears the rows whichever build deletes.
CREATE TABLE IF NOT EXISTS ping_hour (
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  task_id INTEGER NOT NULL REFERENCES ping_task(id) ON DELETE CASCADE,
  ts INTEGER NOT NULL,
  answered INTEGER NOT NULL, lost INTEGER NOT NULL,
  latency INTEGER, lo INTEGER, hi INTEGER,
  PRIMARY KEY (node_id, ts, task_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS session (
  token_hash TEXT    PRIMARY KEY,
  expires_at INTEGER NOT NULL,
  principal_type TEXT NOT NULL DEFAULT 'admin',
  user_id INTEGER
);
"#;

// Desired sing-box state. `node` is this hub's existing machine table. Kept
// separate because open() runs it before the versioned migration for old files.
pub(super) const RESOURCE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS proxies (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  include_node_name INTEGER NOT NULL DEFAULT 0,
  protocol TEXT NOT NULL,
  address_type TEXT NOT NULL,
  address TEXT NOT NULL,
  port INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  flow TEXT NOT NULL DEFAULT '',
  config TEXT NOT NULL DEFAULT '{}',
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_proxies_node_id ON proxies(node_id);

CREATE TABLE IF NOT EXISTS users (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  username TEXT NOT NULL,
  uuid TEXT NOT NULL UNIQUE,
  password_hash TEXT,
  subscription_token TEXT NOT NULL DEFAULT '',
  traffic_limit INTEGER NOT NULL DEFAULT 0,
  device_limit INTEGER NOT NULL DEFAULT 0,
  traffic_reset_day INTEGER NOT NULL DEFAULT 0,
  traffic_reset_period TEXT NOT NULL DEFAULT '',
  auto_authorize_new_proxies INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  expires_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_username ON users(username);

CREATE TABLE IF NOT EXISTS user_proxy_authorizations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE CASCADE,
  enabled INTEGER NOT NULL DEFAULT 1,
  auth TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS user_proxy_order (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE CASCADE,
  sort INTEGER NOT NULL,
  PRIMARY KEY(user_id,proxy_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_user_proxy_authorizations_unique ON user_proxy_authorizations(user_id,proxy_id);
CREATE INDEX IF NOT EXISTS idx_user_proxy_authorizations_user_id ON user_proxy_authorizations(user_id);
CREATE INDEX IF NOT EXISTS idx_user_proxy_authorizations_proxy_id ON user_proxy_authorizations(proxy_id);

CREATE TABLE IF NOT EXISTS proxy_user_traffic (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
  uplink_bytes INTEGER NOT NULL DEFAULT 0 CHECK (uplink_bytes >= 0),
  downlink_bytes INTEGER NOT NULL DEFAULT 0 CHECK (downlink_bytes >= 0),
  last_uplink_counter INTEGER CHECK (last_uplink_counter IS NULL OR last_uplink_counter >= 0),
  last_downlink_counter INTEGER CHECK (last_downlink_counter IS NULL OR last_downlink_counter >= 0),
  last_seen_at INTEGER,
  reset_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, node_id)
);
CREATE INDEX IF NOT EXISTS idx_proxy_user_traffic_node_id ON proxy_user_traffic(node_id);

CREATE TABLE IF NOT EXISTS proxy_node_traffic (
  node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,
  uplink_bytes INTEGER NOT NULL DEFAULT 0 CHECK (uplink_bytes >= 0),
  downlink_bytes INTEGER NOT NULL DEFAULT 0 CHECK (downlink_bytes >= 0),
  last_uplink_counter INTEGER CHECK (last_uplink_counter IS NULL OR last_uplink_counter >= 0),
  last_downlink_counter INTEGER CHECK (last_downlink_counter IS NULL OR last_downlink_counter >= 0),
  last_seen_at INTEGER,
  reset_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
"#;

/// Schema revision this build expects, stamped into `PRAGMA user_version`.
/// Increment it and add a `migrate_to_N` when the schema changes under a
/// database already in service. Every migration must be:
///
/// - Additive: a new column carries a default, and no column an earlier build
///   reads is renamed or dropped. install-hub.sh rolls a hub that fails to start
///   back to the previous binary, which then runs on the migrated file. The
///   version-15 migration intentionally clears user proxy authorizations once;
///   a marker in `setting` prevents a rollback and re-upgrade from clearing new
///   grants a second time.
/// - Safe to run twice: an earlier build stamps its own, lower version into a
///   newer file, and the next upgrade runs the migration again.
///
/// A new column goes into `SCHEMA` as well, for fresh files, but an index on it
/// cannot: `open` runs `SCHEMA` before migrating, and on an older file the
/// column is not there yet. `an_upgraded_release_matches_a_fresh_database`
/// holds every migration to these rules, starting from v1.0.0's schema.
pub(super) const SCHEMA_VERSION: i64 = 21;

/// Adds a column older databases lack. A duplicate column indicates the
/// migration has already run; every other error must propagate.
pub(super) fn add_column(conn: &Connection, table: &str, column: &str) -> Result<()> {
    match conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column}"), []) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("duplicate column name") => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// True when `table`'s stored DDL contains `needle`, which is how a migration
/// determines the shape of the database it inherited.
pub(super) fn schema_mentions(conn: &Connection, table: &str, needle: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name=?1 AND sql LIKE ?2",
        params![table, format!("%{needle}%")],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// One table's column names. `table` is always a [`TABLES`] entry rather than
/// caller-supplied, which is why it can be formatted into the pragma.
pub(super) fn columns_of(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(names.collect::<Result<_, _>>()?)
}

/// Everything accumulated before a version was recorded. Runs once, on a
/// database predating the stamp.
pub(super) fn migrate_to_1(conn: &Connection) -> Result<()> {
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
pub(super) fn migrate_to_2(conn: &Connection) -> Result<()> {
    if schema_mentions(conn, "metric", "load1")? {
        conn.execute("ALTER TABLE metric DROP COLUMN load1", [])?;
        info!("dropped metric.load1; nothing read it");
    }
    Ok(())
}

pub(super) fn migrate_to_3(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country TEXT NOT NULL DEFAULT ''")
}

pub(super) fn migrate_to_4(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "notify INTEGER NOT NULL DEFAULT 0")?;
    add_column(conn, "node", "down_since INTEGER NOT NULL DEFAULT 0")
}

/// Every country stored until now was looked up from `ip`. Recording that
/// keeps the badge of a node whose lookup address is still `ip`, and has a node
/// whose public interface address now takes precedence looked up again at its
/// next hello. The columns set by hand start empty: automatic.
pub(super) fn migrate_to_5(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country_ip TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "country_pin TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "ipv4_pin TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "ipv6_pin TEXT NOT NULL DEFAULT ''")?;
    conn.execute("UPDATE node SET country_ip = ip WHERE country != ''", [])?;
    Ok(())
}

pub(super) fn migrate_to_6(conn: &Connection) -> Result<()> {
    add_column(conn, "ping_task", "auto_join INTEGER NOT NULL DEFAULT 0")
}

pub(super) fn migrate_to_7(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "group_name TEXT NOT NULL DEFAULT ''")
}

pub(super) fn migrate_to_8(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "country_prev_ip TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "node", "country_prev TEXT NOT NULL DEFAULT ''")
}

/// Every existing probe ties at 0, so `ORDER BY sort, id` keeps the id order an
/// upgraded database listed them in.
pub(super) fn migrate_to_9(conn: &Connection) -> Result<()> {
    add_column(conn, "ping_task", "sort INTEGER NOT NULL DEFAULT 0")
}

pub(super) fn migrate_to_10(conn: &Connection) -> Result<()> {
    conn.execute_batch(RESOURCE_SCHEMA)?;
    Ok(())
}

pub(super) fn migrate_to_11(conn: &Connection) -> Result<()> {
    add_column(conn, "proxies", "include_node_name INTEGER NOT NULL DEFAULT 0")
}

/// User identity replaces the per-authorization UUID. Old credentials are
/// intentionally discarded per the operator's instruction; authorization rows
/// (including flow/enabled/timestamps) survive the table rename.
pub(super) fn migrate_to_12(conn: &Connection) -> Result<()> {
    add_column(conn, "users", "uuid TEXT NOT NULL DEFAULT ''")?;
    let ids = {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE uuid='' ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        conn.execute("UPDATE users SET uuid=?2 WHERE id=?1", params![id, uuid_v4()])?;
    }
    conn.execute("CREATE UNIQUE INDEX IF NOT EXISTS idx_users_uuid ON users(uuid)", [])?;

    let old_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='user_proxy_access')",
        [],
        |r| r.get(0),
    )?;
    if old_exists {
        // Version 12's CREATE IF NOT EXISTS may already have made the target
        // table before this migration. Preserve every relationship and its
        // non-UUID settings, stripping uuid out of the old auth JSON.
        conn.execute_batch(
            "INSERT INTO user_proxy_authorizations(id,user_id,proxy_id,enabled,auth,created_at,updated_at)
             SELECT id,user_id,proxy_id,enabled,
                    CASE WHEN json_valid(auth) THEN json_remove(auth,'$.uuid') ELSE '{}' END,
                    created_at,updated_at FROM user_proxy_access;
             DROP TABLE user_proxy_access;",
        )?;
    }
    Ok(())
}

/// Adds web credentials and per-user subscription capabilities. Existing users
/// deliberately receive no password; an administrator must set one before the
/// account can be used to sign in. Existing sessions remain administrator
/// sessions so an upgrade does not log the operator out.
pub(super) fn migrate_to_13(conn: &Connection) -> Result<()> {
    add_column(conn, "users", "password_hash TEXT")?;
    add_column(conn, "users", "subscription_token TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "session", "principal_type TEXT NOT NULL DEFAULT 'admin'")?;
    add_column(conn, "session", "user_id INTEGER")?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_subscription_token
             ON users(subscription_token) WHERE subscription_token != '';",
    )?;
    let ids = {
        let mut stmt = conn.prepare("SELECT id FROM users WHERE subscription_token='' ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in ids {
        conn.execute("UPDATE users SET subscription_token=?2 WHERE id=?1", params![id, random_secret()])?;
    }
    Ok(())
}

pub(super) fn migrate_to_14(conn: &Connection) -> Result<()> {
    conn.execute_batch(RESOURCE_SCHEMA)?;
    Ok(())
}

/// Flow now belongs to a proxy rather than to an individual authorization.
/// The requested reset removes all old grants once; the setting marker survives
/// a binary rollback that stamps an older `user_version` into this file.
pub(super) fn migrate_to_15(conn: &Connection, reset_authorizations: bool) -> Result<()> {
    add_column(conn, "proxies", "flow TEXT NOT NULL DEFAULT ''")?;
    let setting_columns = columns_of(conn, "setting")?;
    // Backup validation compares the migrated schema after this function. Defer
    // a malformed setting table to that gate instead of hiding its diagnosis in
    // a migration SQL error.
    if !setting_columns.contains("key") || !setting_columns.contains("value") {
        return Ok(());
    }
    let reset: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM setting WHERE key='migration_v15_authorizations_reset')",
        [],
        |r| r.get(0),
    )?;
    if !reset {
        if reset_authorizations {
            conn.execute("DELETE FROM user_proxy_authorizations", [])?;
        }
        conn.execute("INSERT INTO setting(key,value) VALUES('migration_v15_authorizations_reset','1')", [])?;
    }
    Ok(())
}

pub(super) fn random_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

pub(super) fn uuid_v4() -> String {
    let mut bytes = rand::random::<[u8; 16]>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],bytes[1],bytes[2],bytes[3],bytes[4],bytes[5],bytes[6],bytes[7],
        bytes[8],bytes[9],bytes[10],bytes[11],bytes[12],bytes[13],bytes[14],bytes[15])
}
/// Rows written before the peak existed hold 0, which `Db::metrics` reads as
/// the row's mean rather than rewriting every row of history here.
pub(super) fn migrate_to_18(conn: &Connection) -> Result<()> {
    add_column(conn, "metric", "net_rx_max INTEGER NOT NULL DEFAULT 0")?;
    add_column(conn, "metric", "net_tx_max INTEGER NOT NULL DEFAULT 0")
}

/// Brings a database already in service up to `SCHEMA_VERSION` and stamps it.
/// `from` is its current version, so a fresh file passes `SCHEMA_VERSION` and
/// receives only the stamp.
///
/// One transaction covers every step and the stamp. SQLite rolls back schema
/// changes and `user_version` alike, so a failure part-way -- a full disk, a
/// killed process -- leaves the file at the version it started from rather than
/// between two.
///
/// Restoring a backup also arrives here: the copy carries its own version and
/// requires the same migrations a restart would have run.
pub(super) fn migrate_to_20(conn: &Connection) -> Result<()> {
    add_column(conn, "users", "auto_authorize_new_proxies INTEGER NOT NULL DEFAULT 0")
}

pub(super) fn migrate(conn: &Connection, from: i64) -> Result<()> {
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
    if from < 14 {
        migrate_to_14(&tx)?;
    }
    migrate_to_15(&tx, from < 15)?;
    add_column(&tx, "proxies", "sort INTEGER NOT NULL DEFAULT 0")?;
    for column in [
        "traffic_limit INTEGER NOT NULL DEFAULT 0",
        "device_limit INTEGER NOT NULL DEFAULT 0",
        "traffic_reset_day INTEGER NOT NULL DEFAULT 0",
        "traffic_reset_period TEXT NOT NULL DEFAULT ''",
    ] {
        add_column(&tx, "users", column)?;
    }
    migrate_to_20(&tx)?;
    migrate_history(&tx)?;
    migrate_public_remark(&tx)?;
    migrate_other_ip(&tx)?;
    seed_default_admin(&tx)?;
    if from < 18 {
        migrate_to_18(&tx)?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_proxy_order (
        user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE CASCADE,
        sort INTEGER NOT NULL,
        PRIMARY KEY(user_id,proxy_id)
    )",
    )?;
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()?;
    Ok(())
}

/// The built-in `admin` resource user exists in every database. Keeping the
/// insert in the migration transaction also covers restored backups, while
/// leaving an existing account and its UUID untouched.
pub(super) fn seed_default_admin(conn: &Connection) -> Result<()> {
    let columns = columns_of(conn, "users")?;
    if !["username", "uuid", "enabled", "expires_at", "created_at", "updated_at", "subscription_token"]
        .iter()
        .all(|column| columns.contains(*column))
    {
        // Backup preflight also runs migrations before comparing table shapes.
        // Let its schema check report malformed `users` tables rather than
        // turning the seed query into an unrelated SQL error.
        return Ok(());
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_subscription_token
             ON users(subscription_token) WHERE subscription_token != '';",
    )?;
    let exists: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE username='admin')", [], |r| r.get(0))?;
    if !exists {
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO users(username,uuid,subscription_token,enabled,expires_at,created_at,updated_at)
             VALUES('admin',?1,?2,1,NULL,?3,?3)",
            params![uuid_v4(), random_secret(), now],
        )?;
    }
    Ok(())
}

/// Every table a backup must carry before this build will restore it.
pub(super) const TABLES: [&str; 16] = [
    "setting",
    "node",
    "proxies",
    "users",
    "user_proxy_authorizations",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
    "proxy_user_traffic",
    "proxy_node_traffic",
    "user_proxy_order",
    "metric_hour",
    "ping_hour",
];
pub(super) const PRE_RESOURCE_TABLES: [&str; 8] =
    ["setting", "node", "traffic", "metric", "ping_task", "ping_node", "ping_record", "session"];
pub(super) const PRE_PROXY_TRAFFIC_TABLES: [&str; 11] = [
    "setting",
    "node",
    "proxies",
    "users",
    "user_proxy_authorizations",
    "traffic",
    "metric",
    "ping_task",
    "ping_node",
    "ping_record",
    "session",
];

/// The hourly tier, and the peak CPU the minute rows begin to carry. A file in
/// service already has both tables, since `open` runs `SCHEMA` first; a backup
/// from an earlier release does not, and restoring one would leave every rollup
/// failing until the next restart.
fn migrate_history(conn: &Connection) -> Result<()> {
    add_column(conn, "metric", "cpu_max REAL NOT NULL DEFAULT 0")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS metric_hour (
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           ts      INTEGER NOT NULL,
           minutes INTEGER NOT NULL,
           cpu REAL NOT NULL, cpu_max REAL NOT NULL,
           mem_used INTEGER NOT NULL, swap_used INTEGER NOT NULL, disk_used INTEGER NOT NULL,
           net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL,
           net_rx_max INTEGER NOT NULL, net_tx_max INTEGER NOT NULL,
           tcp INTEGER NOT NULL, udp INTEGER NOT NULL, procs INTEGER NOT NULL,
           PRIMARY KEY (node_id, ts)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS ping_hour (
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           task_id INTEGER NOT NULL REFERENCES ping_task(id) ON DELETE CASCADE,
           ts INTEGER NOT NULL,
           answered INTEGER NOT NULL, lost INTEGER NOT NULL,
           latency INTEGER, lo INTEGER, hi INTEGER,
           PRIMARY KEY (node_id, ts, task_id)
         ) WITHOUT ROWID;",
    )?;
    Ok(())
}

pub(super) fn migrate_public_remark(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "public_remark TEXT NOT NULL DEFAULT ''")
}

pub(super) fn migrate_other_ip(conn: &Connection) -> Result<()> {
    add_column(conn, "node", "ip_other TEXT NOT NULL DEFAULT ''")
}
