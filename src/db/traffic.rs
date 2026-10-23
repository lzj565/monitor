use super::*;

impl Db {
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
        booted: Option<i64>,
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
            last_seen,
        ) = conn
            .prepare_cached(
                "SELECT t.boot_id, t.last_rx, t.last_tx, t.total_rx, t.total_tx, t.month_rx, t.month_tx,
                    t.month_start, t.day_rx, t.day_tx, t.day_start, n.traffic_reset_day, n.last_seen
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
                    r.get::<_, i64>(12)?,
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
        // indistinguishably from the epoch alone, that a second machine shares
        // the token. Booking the reading would cost hundreds of gigabytes
        // against a total that only increases.
        //
        // Except for a machine that booted after the hub last heard from this
        // token: its counters started from zero while unobserved, so the reading
        // is exactly the bytes since boot -- a reboot during a hub outage would
        // otherwise lose the whole outage. A second machine sharing the token
        // booted before the other's last report, and an epoch whose kernel part
        // is unchanged is a different set of interfaces, not a restart. The
        // bytes between the last booking and the reboot remain lost.
        // `last_seen` trails the last report by up to a minute, so a machine
        // booted within it books its own bytes since boot, once; the next
        // report from the other moves `last_seen` past both boots.
        let unseen_boot = !prev_boot.is_empty()
            && prev_boot.split('/').next() != boot_id.split('/').next()
            && booted.is_some_and(|b| last_seen > 0 && b > last_seen);
        let (d_rx, d_tx) = if unseen_boot {
            info!("node {node_id} restarted after it was last seen; booking its counters since boot");
            (rx, tx)
        } else if prev_boot.is_empty() || prev_boot != boot_id {
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
               (node_id, ts, cpu, mem_used, swap_used, disk_used, net_rx, net_tx, tcp, udp, procs,
                net_rx_max, net_tx_max, cpu_max)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
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
                n("procs"),
                n("net_rx_max"),
                n("net_tx_max"),
                f("cpu_max")
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
    ///
    /// `net_rx_max` and `net_tx_max` are the bucket's highest rather than its
    /// mean, since a maximum of maxima loses nothing: a week's window peaks at
    /// the same rate as the minute that reached it. Each row counts as at least
    /// its own mean: rows predating the column hold 0, and the mean, timed by
    /// the hub's arrivals rather than the agent's clock, can edge past the
    /// agent's own rates by the network's jitter.
    pub fn metrics(&self, node_id: i64, span: Span) -> Result<Vec<serde_json::Value>> {
        let mut reader = self.reader();
        // One snapshot for every statement below. On the reader, a rollup can
        // otherwise commit between reading the watermark and reading the rows
        // on either side of it.
        let conn = reader.transaction()?;
        let row = |r: &rusqlite::Row<'_>| {
            Ok(serde_json::json!({
                "ts": r.get::<_, i64>(0)?, "cpu": r.get::<_, f64>(1)?,
                "mem_used": r.get::<_, i64>(2)?, "disk_used": r.get::<_, i64>(3)?,
                "net_rx": r.get::<_, i64>(4)?, "net_tx": r.get::<_, i64>(5)?,
                "net_rx_max": r.get::<_, i64>(6)?, "net_tx_max": r.get::<_, i64>(7)?,
                "cpu_max": r.get::<_, f64>(8)?, "minutes": r.get::<_, i64>(9)?,
                "swap_used": r.get::<_, i64>(10)?, "tcp": r.get::<_, i64>(11)?,
                "udp": r.get::<_, i64>(12)?, "procs": r.get::<_, i64>(13)?,
            }))
        };
        if !span.hourly {
            let mut stmt = conn.prepare_cached(
                "SELECT (MIN(ts)/?3)*?3, AVG(cpu), CAST(AVG(mem_used) AS INTEGER),
                        CAST(AVG(disk_used) AS INTEGER),
                        CAST(AVG(net_rx) AS INTEGER), CAST(AVG(net_tx) AS INTEGER),
                        MAX(MAX(net_rx, net_rx_max)), MAX(MAX(net_tx, net_tx_max)),
                        MAX(MAX(cpu, cpu_max)), COUNT(*), CAST(AVG(swap_used) AS INTEGER),
                        CAST(ROUND(AVG(tcp)) AS INTEGER), CAST(ROUND(AVG(udp)) AS INTEGER),
                        CAST(ROUND(AVG(procs)) AS INTEGER)
                 FROM metric WHERE node_id=?1 AND ts>=?2 GROUP BY ts/?3 ORDER BY ts/?3",
            )?;
            let rows = stmt.query_map(params![node_id, span.since, span.step], row)?;
            return Ok(rows.collect::<Result<_, _>>()?);
        }
        let rolled = rolled(&conn)?.unwrap_or(i64::MIN);
        let mut stmt = conn.prepare_cached(
            "SELECT (MIN(ts)/?3)*?3, SUM(cpu*w)/SUM(w), SUM(mem_used*w)/SUM(w), SUM(disk_used*w)/SUM(w),
                    SUM(net_rx*w)/SUM(w), SUM(net_tx*w)/SUM(w), MAX(rx_max), MAX(tx_max), MAX(cpu_top), SUM(w),
                    SUM(swap_used*w)/SUM(w), CAST(ROUND(SUM(tcp*w)*1.0/SUM(w)) AS INTEGER),
                    CAST(ROUND(SUM(udp*w)*1.0/SUM(w)) AS INTEGER),
                    CAST(ROUND(SUM(procs*w)*1.0/SUM(w)) AS INTEGER)
             FROM (SELECT ts, minutes AS w, cpu, mem_used, disk_used, net_rx, net_tx,
                          net_rx_max AS rx_max, net_tx_max AS tx_max, cpu_max AS cpu_top,
                          swap_used, tcp, udp, procs
                   FROM metric_hour WHERE node_id=?1 AND ts>=?2 AND ts<?4
                   UNION ALL
                   SELECT ts, 1, cpu, mem_used, disk_used, net_rx, net_tx,
                          MAX(net_rx, net_rx_max), MAX(net_tx, net_tx_max), MAX(cpu, cpu_max),
                          swap_used, tcp, udp, procs
                   FROM metric WHERE node_id=?1
                        AND ts>=MAX(?2, ?4, (SELECT MAX(ts) FROM metric WHERE node_id=?1) - ?5))
             GROUP BY ts/?3 ORDER BY ts/?3",
        )?;
        let rows =
            stmt.query_map(params![node_id, span.since, span.step, rolled, DETAIL_DAYS * 86_400], row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Drops history beyond the retention window. Traffic totals live in their
    /// own table precisely so history can be pruned freely.
    pub fn prune(&self, keep_days: i64) -> Result<usize> {
        let now = Utc::now().timestamp();
        let nodes: Vec<i64> = self
            .conn()
            .prepare_cached("SELECT id FROM node")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        let minutes = now - keep_days.min(DETAIL_DAYS) * 86_400;
        let hours = now - keep_days * 86_400;
        let mut pruned = 0;
        for id in nodes {
            for (table, cutoff, guarded) in [
                ("metric", minutes, true),
                ("ping_record", minutes, true),
                ("metric_hour", hours, false),
                ("ping_hour", hours, false),
            ] {
                loop {
                    if halted() {
                        return Ok(pruned);
                    }
                    let conn = self.conn();
                    let before =
                        if guarded { cutoff.min(rolled(&conn)?.unwrap_or(i64::MIN)) } else { cutoff };
                    let first: Option<i64> = conn
                        .prepare_cached(&format!("SELECT MIN(ts) FROM {table} WHERE node_id=?1"))?
                        .query_row([id], |r| r.get(0))?;
                    let Some(first) = first.filter(|&ts| ts < before) else { break };
                    pruned += conn
                        .prepare_cached(&format!("DELETE FROM {table} WHERE node_id=?1 AND ts<?2"))?
                        .execute(params![id, if guarded { before.min(first + 86_400) } else { before }])?;
                    drop(conn);
                    let_waiters_in();
                }
            }
        }
        Ok(pruned)
    }
}

impl Db {
    // ---- the hourly tier ----

    /// Folds every hour whose rows are complete into the hourly tier and
    /// returns how many were folded.
    ///
    /// One hour per transaction, so a long catch-up -- the first run after an
    /// upgrade folds every hour still held in minute rows -- lets the agents'
    /// writes through between hours. At 100 nodes and four probes an hour takes
    /// about 15 ms.
    ///
    /// Without a watermark it records one at the hour of the oldest minute row,
    /// or with none at all at the current hour, so that `prune` has a watermark
    /// to respect. Hours before `keep_days` are skipped rather than folded, as
    /// `prune` would drop them next.
    pub fn roll_up(&self, now: i64, keep_days: i64) -> Result<usize> {
        {
            let conn = self.conn();
            if rolled(&conn)?.is_none() {
                let from = oldest(&conn, &["metric", "ping_record"])?.unwrap_or(now);
                conn.execute(
                    "INSERT OR REPLACE INTO setting (key, value) VALUES (?1, ?2)",
                    params![ROLLED, (from.div_euclid(3_600) * 3_600).to_string()],
                )?;
            }
        }
        let floor = (now - keep_days * 86_400).div_euclid(3_600) * 3_600;
        let mut folded = 0;
        while !halted() && self.fold_next(floor, now - 3_600 - LATE)? {
            let_waiters_in();
            folded += 1;
        }
        Ok(folded)
    }
}

impl Db {
    /// Folds the hour at the watermark, or at `floor` when the watermark is
    /// older, into `metric_hour` and `ping_hour` and moves the watermark past
    /// it, in one transaction: a failure leaves the hour to be folded again
    /// rather than half folded. `INSERT OR REPLACE` makes a second fold of the
    /// same hour a rewrite. False, folding nothing, once the hour is past `last`
    /// or when there is no watermark.
    ///
    /// The watermark is read here rather than carried between hours: a restore
    /// replaces it along with the rows it describes, and a stale one would skip
    /// the restored hours.
    ///
    /// By node, as the keys begin with it; `ts` alone would scan each table.
    /// The probe results are held one node's hour at a time, and only for
    /// probes that still exist: `ping_hour` refers to `ping_task`, and one row
    /// left behind by a deleted probe would otherwise fail the hour on every
    /// pass, keeping its minute rows from ever being pruned.
    ///
    /// The byte columns truncate their means, a loss under one byte; the counts
    /// round theirs, as truncating would drop a UDP socket held for half the
    /// hour.
    fn fold_next(&self, floor: i64, last: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(hour) = rolled(&tx)?.map(|h| h.max(floor)).filter(|&h| h <= last) else {
            return Ok(false);
        };
        tx.prepare_cached(
            "INSERT OR REPLACE INTO metric_hour
               (node_id, ts, minutes, cpu, cpu_max, mem_used, swap_used, disk_used, net_rx, net_tx,
                net_rx_max, net_tx_max, tcp, udp, procs)
             SELECT node_id, ?1, COUNT(*), AVG(cpu), MAX(MAX(cpu, cpu_max)), CAST(AVG(mem_used) AS INTEGER),
                    CAST(AVG(swap_used) AS INTEGER), CAST(AVG(disk_used) AS INTEGER),
                    CAST(AVG(net_rx) AS INTEGER), CAST(AVG(net_tx) AS INTEGER),
                    MAX(MAX(net_rx, net_rx_max)), MAX(MAX(net_tx, net_tx_max)),
                    CAST(ROUND(AVG(tcp)) AS INTEGER), CAST(ROUND(AVG(udp)) AS INTEGER),
                    CAST(ROUND(AVG(procs)) AS INTEGER)
             FROM metric WHERE node_id IN (SELECT id FROM node) AND ts>=?1 AND ts<?1+3600
             GROUP BY node_id",
        )?
        .execute([hour])?;
        let nodes: Vec<i64> = tx
            .prepare_cached("SELECT id FROM node")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for node in nodes {
            let mut probes: std::collections::BTreeMap<i64, Tally> = Default::default();
            let mut read = tx.prepare_cached(
                "SELECT task_id, latency FROM ping_record
                 WHERE node_id=?1 AND ts>=?2 AND ts<?2+3600 AND task_id IN (SELECT id FROM ping_task)",
            )?;
            let mut rows = read.query(params![node, hour])?;
            while let Some(r) = rows.next()? {
                probes.entry(r.get(0)?).or_default().add(Sample::result(r.get(1)?));
            }
            let mut write = tx.prepare_cached(
                "INSERT OR REPLACE INTO ping_hour (node_id, task_id, ts, answered, lost, latency, lo, hi)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (task, mut t) in probes {
                let median = t.median();
                write.execute(params![node, task, hour, t.answered(), t.lost, median, t.lo, t.hi])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO setting (key, value) VALUES (?1, ?2)",
            params![ROLLED, (hour + 3_600).to_string()],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
