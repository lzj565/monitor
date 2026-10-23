use super::*;

impl Db {
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
            let id = next_id(&tx, "ping_task")?;
            tx.execute(
                "INSERT INTO ping_task (id, name, target, interval, auto_join, sort)
                 VALUES (?5,?1,?2,?3,?4,(SELECT COALESCE(MAX(sort),-1)+1 FROM ping_task))",
                params![t.name, t.target, t.interval, t.auto_join, id],
            )?;
            id
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
                Some(rusqlite::ErrorCode::ConstraintViolation) => {
                    anyhow::Error::from(e).context(crate::Shown(format!("节点 {node} 不存在，可能已被删除")))
                }
                _ => e.into(),
            })?;
        }
        Self::refuse_crowded(&tx, None)?;
        // The next node created receives every auto-joining probe at once.
        let joining: i64 =
            tx.query_row("SELECT COUNT(*) FROM ping_task WHERE auto_join", [], |r| r.get(0))?;
        if joining > Self::MAX_PROBES_PER_NODE {
            refuse!("新节点会自动加入 {joining} 个探测任务，agent 最多只跑 {} 个", Self::MAX_PROBES_PER_NODE);
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
        let conn = self.reader();
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
        span: Span,
    ) -> Result<(Vec<serde_json::Value>, serde_json::Value)> {
        let mut reader = self.reader();
        // One snapshot for every statement below, as in `metrics`.
        let conn = reader.transaction()?;
        let step = span.step;
        let mut out = Vec::new();
        // Per probe in the bucket being filled.
        let mut open: Vec<(i64, Tally)> = Vec::new();
        // Per probe across the whole window: how many were lost, out of how many.
        // Folded in the same pass rather than queried from SQLite a second time,
        // for the same reason the bucket fold itself is in Rust.
        let mut totals: HashMap<i64, (i64, i64)> = HashMap::new();
        let mut bucket = 0;
        let mut feed = |b: i64, task: i64, sample: Sample| {
            if b != bucket {
                close_bucket(&mut out, &mut open, bucket * step);
                bucket = b;
            }
            let seen = totals.entry(task).or_insert((0, 0));
            seen.0 += sample.lost;
            seen.1 += sample.answered + sample.lost;
            match open.iter_mut().find(|(id, _)| *id == task) {
                Some((_, tally)) => tally.add(sample),
                None => {
                    let mut tally = Tally::default();
                    tally.add(sample);
                    open.push((task, tally));
                }
            }
        };
        // The folded hours first, then the minute rows after them: both come off
        // their keys in time order, and a bucket spanning the watermark takes
        // rows from each.
        let mut minutes_from = span.since;
        if span.hourly {
            let rolled = rolled(&conn)?.unwrap_or(i64::MIN);
            let newest: Option<i64> = conn
                .prepare_cached("SELECT MAX(ts) FROM ping_record WHERE node_id=?1")?
                .query_row([node_id], |r| r.get(0))?;
            minutes_from = minutes_from.max(rolled).max(newest.unwrap_or(0) - DETAIL_DAYS * 86_400);
            let mut stmt = conn.prepare_cached(PING_HOURS)?;
            let mut rows = stmt.query(params![node_id, span.since, step, rolled])?;
            while let Some(r) = rows.next()? {
                let sample = Sample {
                    answered: r.get(2)?,
                    lost: r.get(3)?,
                    median: r.get(4)?,
                    lo: r.get(5)?,
                    hi: r.get(6)?,
                };
                feed(r.get(0)?, r.get(1)?, sample);
            }
        }
        let mut stmt = conn.prepare_cached(PING_ROWS)?;
        let mut rows = stmt.query(params![node_id, minutes_from, step])?;
        while let Some(r) = rows.next()? {
            feed(r.get(0)?, r.get(1)?, Sample::result(r.get(2)?));
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
        // Sorted after releasing the reader, which the next chart request waits
        // on. A probe missing from the rank, which the assignment filter in
        // `PING_ROWS` rules out today, goes last rather than taking the first
        // colour.
        drop(rows);
        drop(stmt);
        drop(conn);
        drop(reader);
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
}

impl Db {
    /// Refuses a write that leaves some node over [`Self::MAX_PROBES_PER_NODE`],
    /// or `node` alone when the write touched only that node's rows: another
    /// node already over the limit is not this edit's to fix.
    ///
    /// Queried from the table after the rows are in rather than counted from the
    /// request: an update changes the caller's own assignments, so arithmetic on
    /// the way in would have to subtract them again. Run inside the write's
    /// transaction, so refusing rolls it back. By name: the panel identifies
    /// nodes by name and never shows an id.
    fn refuse_crowded(tx: &rusqlite::Transaction, node: Option<i64>) -> Result<()> {
        let crowded: Option<String> = tx
            .query_row(
                "SELECT n.name FROM ping_node p JOIN node n ON n.id = p.node_id
                 WHERE ?2 IS NULL OR p.node_id = ?2
                 GROUP BY p.node_id HAVING COUNT(*) > ?1 LIMIT 1",
                params![Self::MAX_PROBES_PER_NODE, node],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(node) = crowded {
            refuse!(
                "节点「{node}」会被分配超过 {} 个探测任务，agent 最多只跑这么多，多出来的会被静默丢掉",
                Self::MAX_PROBES_PER_NODE
            );
        }
        Ok(())
    }
}

impl Db {
    /// One node's probes, edited from the node's side: `save_ping_task` with
    /// the roles swapped. Only the change from `base` is applied, as there, so a
    /// probe assigned to this node elsewhere while the editor was open keeps the
    /// assignment. `false` when the node does not exist.
    pub fn set_node_ping_tasks(&self, node: i64, tasks: &[i64], base: &[i64]) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if tx.query_row("SELECT 1 FROM node WHERE id=?1", [node], |_| Ok(())).optional()?.is_none() {
            return Ok(false);
        }
        for task in base.iter().filter(|t| !tasks.contains(t)) {
            tx.execute("DELETE FROM ping_node WHERE task_id=?1 AND node_id=?2", params![task, node])?;
        }
        for task in tasks.iter().filter(|t| !base.contains(t)) {
            // The node was checked above, so a key failing here is the probe.
            tx.execute(
                "INSERT OR IGNORE INTO ping_node (task_id, node_id) VALUES (?1,?2)",
                params![task, node],
            )
            .map_err(|e| match e.sqlite_error_code() {
                Some(rusqlite::ErrorCode::ConstraintViolation) => {
                    anyhow::Error::from(e).context(crate::Shown("有监控已被删除，刷新后重试".into()))
                }
                _ => e.into(),
            })?;
        }
        Self::refuse_crowded(&tx, Some(node))?;
        tx.commit()?;
        Ok(true)
    }
}
