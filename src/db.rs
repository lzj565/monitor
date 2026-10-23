//! SQLite storage. A single writer connection behind a mutex: at a handful of
//! nodes reporting every few seconds, every statement here is sub-millisecond.
// ponytail: single global connection; move to a read pool if the dashboard ever
// blocks behind ingest.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, TryLockError};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::runtime::RuntimeFlavor;
use tracing::{info, warn};

pub struct Db {
    /// A read-only connection to the same file, for the history charts. A week
    /// of one node's probe results is a scan of 98 ms at four 60-second probes
    /// and 486 ms at eight 10-second ones. With four of the latter in flight on
    /// `conn`, every agent report and panel request behind them would wait 1.1 s
    /// at the median; with the scans here they wait 1.8 ms, as WAL lets this
    /// connection read while `conn` commits. `None` for `:memory:`, which a second
    /// connection cannot open.
    ///
    /// Declared before `conn` so that it closes first. The last connection to
    /// close folds the WAL into the database file and deletes it, which a
    /// read-only one cannot do, and a stopped hub would otherwise leave rows in
    /// a -wal that a copy of the database file alone misses.
    // ponytail: one reader, so chart requests queue behind one another, at most
    // `api::HISTORY_SLOTS` deep; a pool if that wait becomes visible.
    reader: Option<Mutex<Connection>>,
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Queried before CREATE TABLE runs: a file with no tables receives the
        // current schema directly rather than the history of how it was reached.
        let fresh = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table'", [], |r| r.get::<_, i64>(0))?
            == 0;
        conn.execute_batch(SCHEMA)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        // Fresh/current schemas already get the idempotent declarations here.
        // Older databases create these tables inside migrate_to_10's transaction.
        if fresh || version >= 10 {
            conn.execute_batch(RESOURCE_SCHEMA)?;
        }
        restrict(path);

        migrate(&conn, if fresh { SCHEMA_VERSION } else { version })?;
        // A file from a release that kept no high marks gets them now, from its
        // rows. Otherwise deleting the newest node or probe before the next one
        // is created would lose its id, and `next_id` would issue it again.
        for table in ID_TABLES {
            put_setting(&conn, &format!("{table}_id_issued"), &issued(&conn, table)?.to_string())?;
        }
        let reader = match main_file(&conn) {
            file if file.is_empty() => None,
            file => Some(Mutex::new(read_only(&file)?)),
        };
        Ok(Self { reader, conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        lock(&self.conn)
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
        put_setting(&self.conn(), key, value)
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
}

mod realm;
mod realm_traffic;
pub(super) use realm::*;
use realm_traffic::*;
mod maintenance;
mod probes;
mod resources;
mod schema;
mod sessions;
mod traffic;
use schema::*;
mod models;
pub(super) use models::*;
mod rows;
pub(super) use rows::*;

#[cfg(test)]
mod tests;

/// A string an agent reported, cut down to what may be shown. Control
/// characters break the panel's rows, and the characters markup is made of are
/// dropped because the value reaches a theme from a machine nobody has vouched
/// for: a theme that sets it as HTML would otherwise run a node's script in the
/// hub's origin, beside the administrator's session. No real OS name, kernel,
/// CPU model or host name contains any of them. `chars()` rather than bytes, so
/// the cut falls on a character boundary.
pub fn reported_text(raw: &str, max: usize) -> String {
    raw.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | '"' | '\'')).take(max).collect()
}

impl Db {
    /// The connection the history charts read through: the read-only one, or
    /// the writer for `:memory:`.
    ///
    /// A scan holds its snapshot throughout, and with chart requests queued the
    /// next begins as the last ends, so no commit's checkpoint would find the
    /// reader idle and the WAL would keep every commit for as long as anyone
    /// sent them: 20 MB in 20 s at 100 commits a second, from four requests kept
    /// in flight. Before the scan the reader is idle, and a checkpoint taken
    /// here resets the WAL itself. Past 4 MiB only, as without readers the WAL
    /// stays within the 1 MiB `journal_size_limit` plus one transaction.
    ///
    /// Without waiting: an export is the one other reader, and waiting on it
    /// would hold the writer, and every agent report behind it, for the busy
    /// timeout.
    ///
    /// The writer is taken after the reader here and must never be held while
    /// taking the reader.
    fn reader(&self) -> MutexGuard<'_, Connection> {
        let Some(reader) = &self.reader else { return self.conn() };
        let reader = reader.lock().unwrap_or_else(|e| e.into_inner());
        if bytes_of(&format!("{}-wal", reader.path().unwrap_or_default())) > 4 << 20 {
            let conn = self.conn();
            let _ = conn.busy_timeout(std::time::Duration::ZERO);
            let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        }
        reader
    }
}
