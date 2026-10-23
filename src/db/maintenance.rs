use super::*;

impl Db {
    // ---- the database file itself ----

    /// The file this connection is open on, empty for `:memory:`.
    pub fn file(&self) -> String {
        main_file(&self.conn())
    }

    /// The retention window used by both `prune` and the data page. Stored as
    /// text by the settings form, so a missing or unparsable value falls back to
    /// the default rather than erroring.
    pub fn retention_days(&self) -> i64 {
        self.get("retention_days")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(DEFAULT_RETENTION_DAYS)
            .clamp(1, MAX_RETENTION_DAYS)
    }

    /// What the panel's data page reads: how much space the file occupies, how
    /// much of that is free pages awaiting a `VACUUM`, and how far back the
    /// history actually reaches.
    ///
    /// `oldest` against `retention` is the one pair here that can indicate a
    /// fault: history older than the window means `prune` has not been running.
    pub fn stats(&self) -> Result<serde_json::Value> {
        // Before acquiring the connection, which for `:memory:` is the writer
        // that `retention_days` acquires as well.
        let retention = self.retention_days();
        // A read-only connection of its own: the counts scan every row of both
        // tiers of history, which on a cold cache means reading most of the
        // file -- 13 s for 52 MB at 4 MB/s. Through the writer, agent reports
        // would wait out that read; through the charts' reader, so would the
        // public page's history charts.
        let (own, writer);
        let conn: &Connection = if self.reader.is_some() {
            own = read_only(&self.file())?;
            &own
        } else {
            writer = self.conn();
            &writer
        };
        let file = main_file(conn);
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let free_pages: i64 = conn.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        // Across both tiers: history begins at whichever row is earliest, and
        // a minute row can predate the hourly tier while a rollup catches up.
        let oldest = oldest(conn, &["metric_hour", "ping_hour", "metric", "ping_record"])?;
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
            // What the panel shows, counted on the hub's clock: from the browser's,
            // a clock eight hours off would shift the span by up to a day.
            "oldest_ago": oldest.map(|t| (Utc::now().timestamp() - t).max(0)),
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
        // A connection of its own. `VACUUM INTO` only reads, and WAL allows it to
        // read a consistent snapshot while the agents continue writing through
        // the first -- exporting is the one heavy operation here that need not
        // block them. Not the charts' reader, which it would hold throughout.
        read_only(&self.file())?.execute("VACUUM INTO ?1", [dest])?;
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
        // V1.3/schema 9 backups predate these three tables; their migration
        // creates them. Newer backups must carry every resource table already.
        let required_tables: &[&str] = if version < 10 {
            &PRE_RESOURCE_TABLES
        } else if version < 14 {
            &PRE_PROXY_TRAFFIC_TABLES
        } else if version < 19 {
            &TABLES[..13]
        } else {
            &TABLES
        };
        for table in required_tables.iter().filter(|t| !HOURLY_TABLES.contains(t)) {
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
        reference.execute_batch(RESOURCE_SCHEMA)?;
        migrate(&reference, SCHEMA_VERSION)?;
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
        let live = ID_TABLES.map(|table| issued(&conn, table));
        conn.restore(rusqlite::MAIN_DB, src, None::<fn(rusqlite::backup::Progress)>)?;
        for (table, live) in ID_TABLES.into_iter().zip(live) {
            let kept = live.and_then(|live| {
                let id = issued(&conn, table)?.max(live);
                put_setting(&conn, &format!("{table}_id_issued"), &id.to_string())
            });
            if let Err(e) = kept {
                warn!("keeping the {table} id high mark across the restore failed: {e:#}");
            }
        }
        Ok(())
    }
}
