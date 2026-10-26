use super::*;

impl Db {
    // ---- desired proxy and user state ----

    pub fn all_proxies(&self) -> Result<Vec<Proxy>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id,node_id,name,include_node_name,protocol,address_type,address,port,enabled,flow,config,created_at,updated_at,sort
             FROM proxies ORDER BY sort,id",
        )?;
        let rows = stmt.query_map([], row_to_proxy)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn proxies_for_node(&self, node_id: i64) -> Result<Vec<Proxy>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id,node_id,name,include_node_name,protocol,address_type,address,port,enabled,flow,config,created_at,updated_at,sort
             FROM proxies WHERE node_id=?1 ORDER BY sort,id",
        )?;
        let rows = stmt.query_map([node_id], row_to_proxy)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn proxy(&self, id: i64) -> Result<Option<Proxy>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id,node_id,name,include_node_name,protocol,address_type,address,port,enabled,flow,config,created_at,updated_at,sort
                 FROM proxies WHERE id=?1",
                [id],
                row_to_proxy,
            )
            .optional()?)
    }

    /// Port uniqueness is a service rule, but it is checked under the same
    /// writer lock as the insert/update so concurrent requests cannot race.
    pub fn create_proxy(
        &self,
        node_id: i64,
        draft: &ProxyDraft,
    ) -> Result<std::result::Result<i64, ProxyWriteIssue>> {
        let config = serde_json::to_string(&draft.config)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let node_exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM node WHERE id=?1)", [node_id], |r| r.get(0))?;
        if !node_exists {
            return Ok(Err(ProxyWriteIssue::NodeNotFound));
        }
        if draft.protocol == "mieru"
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM proxies WHERE node_id=?1 AND protocol='mieru' AND id<>?2)",
                params![node_id, -1],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Ok(Err(ProxyWriteIssue::MieruInstanceConflict));
        }
        if proxy_name_in_use(&tx, node_id, draft, None)? {
            return Ok(Err(ProxyWriteIssue::NameConflict));
        }
        if draft.enabled && port_in_use(&tx, node_id, draft.port, None)? {
            return Ok(Err(ProxyWriteIssue::PortConflict));
        }
        let now = Utc::now().timestamp();
        let sort: i64 = tx.query_row("SELECT COALESCE(MAX(sort),-1)+1 FROM proxies", [], |row| row.get(0))?;
        tx.execute(
            "INSERT INTO proxies(node_id,name,include_node_name,protocol,address_type,address,port,enabled,flow,config,created_at,updated_at,sort)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11,?12)",
            params![node_id, draft.name, draft.include_node_name, draft.protocol, draft.address_type,
                    draft.address, draft.port, draft.enabled, draft.flow.as_deref().unwrap_or(""), config, now, sort],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO user_proxy_authorizations(user_id,proxy_id,enabled,auth,created_at,updated_at)
             SELECT id,?1,1,'{}',?2,?2 FROM users WHERE auto_authorize_new_proxies=1",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(Ok(id))
    }

    pub fn update_proxy(
        &self,
        id: i64,
        target_node_id: Option<i64>,
        draft: &ProxyDraft,
    ) -> Result<std::result::Result<(), ProxyWriteIssue>> {
        let config = serde_json::to_string(&draft.config)?;
        let conn = self.conn();
        let current_node_id: Option<i64> =
            conn.query_row("SELECT node_id FROM proxies WHERE id=?1", [id], |r| r.get(0)).optional()?;
        let Some(current_node_id) = current_node_id else {
            return Ok(Err(ProxyWriteIssue::ProxyNotFound));
        };
        let node_id = target_node_id.unwrap_or(current_node_id);
        let node_exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM node WHERE id=?1)", [node_id], |r| r.get(0))?;
        if !node_exists {
            return Ok(Err(ProxyWriteIssue::NodeNotFound));
        }
        if draft.protocol == "mieru"
            && conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM proxies WHERE node_id=?1 AND protocol='mieru' AND id<>?2)",
                params![node_id, id],
                |r| r.get::<_, bool>(0),
            )?
        {
            return Ok(Err(ProxyWriteIssue::MieruInstanceConflict));
        }
        if proxy_name_in_use(&conn, node_id, draft, Some(id))? {
            return Ok(Err(ProxyWriteIssue::NameConflict));
        }
        if draft.enabled && port_in_use(&conn, node_id, draft.port, Some(id))? {
            return Ok(Err(ProxyWriteIssue::PortConflict));
        }
        conn.execute(
            "UPDATE proxies SET node_id=?2,name=?3,include_node_name=?4,protocol=?5,address_type=?6,address=?7,port=?8,
                                enabled=?9,flow=COALESCE(?10,flow),config=?11,updated_at=?12 WHERE id=?1",
            params![
                id,
                node_id,
                draft.name,
                draft.include_node_name,
                draft.protocol,
                draft.address_type,
                draft.address,
                draft.port,
                draft.enabled,
                draft.flow,
                config,
                Utc::now().timestamp()
            ],
        )?;
        Ok(Ok(()))
    }

    pub fn delete_proxy(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM proxies WHERE id=?1", [id])? > 0)
    }

    pub fn users(&self, enabled: Option<bool>, query: Option<&str>) -> Result<Vec<User>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT u.id,u.username,u.uuid,u.enabled,u.expires_at,u.created_at,u.updated_at,
                    (SELECT COUNT(*) FROM user_proxy_authorizations a WHERE a.user_id=u.id),
                    u.traffic_limit,u.device_limit,u.traffic_reset_day,u.auto_authorize_new_proxies
             FROM users u WHERE (?1 IS NULL OR u.enabled=?1) ORDER BY u.id",
        )?;
        let rows = stmt.query_map([enabled], row_to_user)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else { return Ok(rows) };
        let query = query.to_lowercase();
        Ok(rows.into_iter().filter(|u| u.username.to_lowercase().contains(&query)).collect())
    }

    pub fn user(&self, id: i64) -> Result<Option<User>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT u.id,u.username,u.uuid,u.enabled,u.expires_at,u.created_at,u.updated_at,
                        (SELECT COUNT(*) FROM user_proxy_authorizations a WHERE a.user_id=u.id),
                    u.traffic_limit,u.device_limit,u.traffic_reset_day,u.auto_authorize_new_proxies
                 FROM users u WHERE u.id=?1",
                [id],
                row_to_user,
            )
            .optional()?)
    }

    pub fn proxy_user_traffic(&self) -> Result<Vec<ProxyUserTrafficSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT u.id,u.username,COALESCE(SUM(t.uplink_bytes),0),COALESCE(SUM(t.downlink_bytes),0),
                    MAX(t.last_seen_at),MAX(t.reset_at),u.traffic_limit
             FROM users u LEFT JOIN proxy_user_traffic t ON t.user_id=u.id
             GROUP BY u.id ORDER BY u.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProxyUserTrafficSummary {
                    user_id: r.get(0)?,
                    username: r.get(1)?,
                    uplink_bytes: r.get(2)?,
                    downlink_bytes: r.get(3)?,
                    last_seen_at: r.get(4)?,
                    reset_at: r.get(5)?,
                    traffic_limit: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn proxy_user_traffic_by_node(&self, user_id: i64) -> Result<Vec<ProxyUserTrafficByNode>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT t.user_id,t.node_id,n.name,t.uplink_bytes,t.downlink_bytes,t.last_seen_at,t.reset_at
             FROM proxy_user_traffic t JOIN node n ON n.id=t.node_id
             WHERE t.user_id=?1 ORDER BY n.sort,n.id",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok(ProxyUserTrafficByNode {
                    user_id: r.get(0)?,
                    node_id: r.get(1)?,
                    node_name: r.get(2)?,
                    uplink_bytes: r.get(3)?,
                    downlink_bytes: r.get(4)?,
                    last_seen_at: r.get(5)?,
                    reset_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn proxy_node_traffic(&self) -> Result<Vec<ProxyNodeTrafficSummary>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT n.id,n.name,COALESCE(t.uplink_bytes,0),COALESCE(t.downlink_bytes,0),t.last_seen_at,t.reset_at
             FROM node n LEFT JOIN proxy_node_traffic t ON t.node_id=n.id ORDER BY n.sort,n.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProxyNodeTrafficSummary {
                    node_id: r.get(0)?,
                    node_name: r.get(1)?,
                    uplink_bytes: r.get(2)?,
                    downlink_bytes: r.get(3)?,
                    last_seen_at: r.get(4)?,
                    reset_at: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Overview totals use the user table as the single business-traffic
    /// dimension. `active_users` means sampled recently, not necessarily
    /// transferring bytes at this instant.
    pub fn proxy_traffic_overview(&self, active_since: i64) -> Result<ProxyTrafficOverview> {
        let conn = self.conn();
        let (uplink_bytes, downlink_bytes, active_users) = conn.query_row(
            "SELECT COALESCE(SUM(uplink_bytes),0),COALESCE(SUM(downlink_bytes),0),
                    COUNT(DISTINCT CASE WHEN last_seen_at>=?1 THEN user_id END)
             FROM proxy_user_traffic",
            [active_since],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
        )?;
        Ok(ProxyTrafficOverview {
            uplink_bytes,
            downlink_bytes,
            total_bytes: uplink_bytes.saturating_add(downlink_bytes),
            active_users,
        })
    }

    /// Stored user counters at their native `(user_id,node_id)` granularity.
    pub fn proxy_traffic_user_rows(
        &self,
        query: Option<&str>,
        node_id: Option<i64>,
        sort: &str,
        descending: bool,
        page: i64,
        page_size: i64,
    ) -> Result<ProxyTrafficPage<ProxyTrafficUserRow>> {
        let conn = self.conn();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{q}%"));
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_user_traffic t
             JOIN users u ON u.id=t.user_id JOIN node n ON n.id=t.node_id
             WHERE (?1 IS NULL OR t.node_id=?1)
               AND (?2 IS NULL OR u.username LIKE ?2 COLLATE NOCASE OR n.name LIKE ?2 COLLATE NOCASE)",
            params![node_id, pattern],
            |r| r.get(0),
        )?;
        let order_by = traffic_order_by(sort, descending, "t.uplink_bytes", "t.downlink_bytes");
        let sql = format!(
            "SELECT t.user_id,u.username,t.node_id,n.name,t.uplink_bytes,t.downlink_bytes,t.last_seen_at
             FROM proxy_user_traffic t
             JOIN users u ON u.id=t.user_id JOIN node n ON n.id=t.node_id
             WHERE (?1 IS NULL OR t.node_id=?1)
               AND (?2 IS NULL OR u.username LIKE ?2 COLLATE NOCASE OR n.name LIKE ?2 COLLATE NOCASE)
             ORDER BY {order_by},t.user_id,t.node_id LIMIT ?3 OFFSET ?4"
        );
        let mut stmt = conn.prepare(&sql)?;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let items = stmt
            .query_map(params![node_id, pattern, page_size, offset], |r| {
                let uplink_bytes: i64 = r.get(4)?;
                let downlink_bytes: i64 = r.get(5)?;
                Ok(ProxyTrafficUserRow {
                    user_id: r.get(0)?,
                    username: r.get(1)?,
                    node_id: r.get(2)?,
                    node_name: r.get(3)?,
                    uplink_bytes,
                    downlink_bytes,
                    total_bytes: uplink_bytes.saturating_add(downlink_bytes),
                    last_seen_at: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ProxyTrafficPage { items, page, page_size, total })
    }

    /// One row per server hosting proxies, backed by `proxy_node_traffic`.
    /// Protocol metadata is an aggregate of the configured proxies on that
    /// server; traffic itself is never inferred from user counters.
    pub fn proxy_traffic_node_rows(
        &self,
        query: Option<&str>,
        node_id: Option<i64>,
        sort: &str,
        descending: bool,
        page: i64,
        page_size: i64,
    ) -> Result<ProxyTrafficPage<ProxyTrafficNodeRow>> {
        let conn = self.conn();
        let pattern = query.map(str::trim).filter(|q| !q.is_empty()).map(|q| format!("%{q}%"));
        let scope = "(EXISTS(SELECT 1 FROM proxies p0 WHERE p0.node_id=n.id) OR t.node_id IS NOT NULL)";
        let total_sql = format!(
            "SELECT COUNT(*) FROM node n LEFT JOIN proxy_node_traffic t ON t.node_id=n.id
             WHERE {scope} AND (?1 IS NULL OR n.id=?1) AND (?2 IS NULL OR n.name LIKE ?2 COLLATE NOCASE)"
        );
        let total: i64 = conn.query_row(&total_sql, params![node_id, pattern], |r| r.get(0))?;
        let order_by =
            traffic_order_by(sort, descending, "COALESCE(t.uplink_bytes,0)", "COALESCE(t.downlink_bytes,0)");
        let sql = format!(
            "SELECT n.id,n.name,COALESCE(t.uplink_bytes,0),COALESCE(t.downlink_bytes,0),t.last_seen_at,
                    GROUP_CONCAT(DISTINCT p.protocol)
             FROM node n LEFT JOIN proxy_node_traffic t ON t.node_id=n.id
             LEFT JOIN proxies p ON p.node_id=n.id AND p.enabled=1
             WHERE {scope} AND (?1 IS NULL OR n.id=?1) AND (?2 IS NULL OR n.name LIKE ?2 COLLATE NOCASE)
             GROUP BY n.id ORDER BY {order_by},n.sort,n.id LIMIT ?3 OFFSET ?4"
        );
        let mut stmt = conn.prepare(&sql)?;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let mut items: Vec<ProxyTrafficNodeRow> = stmt
            .query_map(params![node_id, pattern, page_size, offset], |r| {
                let uplink_bytes: i64 = r.get(2)?;
                let downlink_bytes: i64 = r.get(3)?;
                let protocols: Option<String> = r.get(5)?;
                Ok(ProxyTrafficNodeRow {
                    proxies: Vec::new(),
                    node_id: r.get(0)?,
                    node_name: r.get(1)?,
                    protocols: protocols
                        .unwrap_or_default()
                        .split(',')
                        .filter(|protocol| !protocol.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    uplink_bytes,
                    downlink_bytes,
                    total_bytes: uplink_bytes.saturating_add(downlink_bytes),
                    last_seen_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut proxies = conn.prepare(
            "SELECT id,name,protocol,enabled,include_node_name FROM proxies WHERE node_id=?1 ORDER BY id",
        )?;
        for row in &mut items {
            row.proxies = proxies
                .query_map([row.node_id], |r| {
                    Ok(TrafficProxySummary {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        protocol: r.get(2)?,
                        enabled: r.get(3)?,
                        include_node_name: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        Ok(ProxyTrafficPage { items, page, page_size, total })
    }

    pub fn reset_due_user_traffic(&self, now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        reset_due_user_traffic(&tx, now)?;
        tx.commit()?;
        Ok(())
    }

    pub fn reset_proxy_user_traffic(&self, user_id: i64, now: i64) -> Result<bool> {
        let conn = self.conn();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user_id], |r| r.get(0))?;
        if !exists {
            return Ok(false);
        }
        conn.execute(
            "UPDATE proxy_user_traffic SET uplink_bytes=0,downlink_bytes=0,reset_at=?2,updated_at=?2
             WHERE user_id=?1",
            params![user_id, now],
        )?;
        Ok(true)
    }

    pub fn reset_proxy_node_traffic(&self, node_id: i64, now: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM node WHERE id=?1)", [node_id], |r| r.get(0))?;
        if !exists {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO proxy_node_traffic(node_id,uplink_bytes,downlink_bytes,reset_at,created_at,updated_at)
             VALUES(?1,0,0,?2,?2,?2)
             ON CONFLICT(node_id) DO UPDATE SET uplink_bytes=0,downlink_bytes=0,reset_at=excluded.reset_at,
                                                updated_at=excluded.updated_at",
            params![node_id, now],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Books one complete Core snapshot atomically. Missing counters for a
    /// currently configured user or inbound are zero; the first observation
    /// establishes a baseline and contributes no historical Core bytes.
    pub fn record_proxy_traffic(
        &self,
        node_id: i64,
        users: &HashMap<String, ProxyTrafficCounter>,
        inbounds: &HashMap<String, ProxyTrafficCounter>,
        now: i64,
    ) -> Result<()> {
        anyhow::ensure!(
            users.values().chain(inbounds.values()).all(|c| c.uplink >= 0 && c.downlink >= 0),
            "Core traffic counters must be non-negative"
        );
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        reset_due_user_traffic(&tx, now)?;
        let proxy_ids = {
            let mut stmt = tx.prepare(
                "SELECT id FROM proxies WHERE node_id=?1 AND enabled=1 AND protocol='vless' ORDER BY id",
            )?;
            let rows = stmt.query_map([node_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let configured_users = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT u.id,u.username FROM users u
                 JOIN user_proxy_authorizations a ON a.user_id=u.id
                 JOIN proxies p ON p.id=a.proxy_id
                 WHERE p.node_id=?1 AND p.protocol='vless' AND p.enabled=1 AND a.enabled=1 AND u.enabled=1
                       AND (u.expires_at IS NULL OR u.expires_at>?2)
                 ORDER BY u.id",
            )?;
            let rows =
                stmt.query_map(params![node_id, now], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut inbound_total = ProxyTrafficCounter::default();
        for proxy_id in proxy_ids {
            let tag = format!("proxy-{proxy_id}");
            let reading = inbounds.get(&tag).copied().unwrap_or_default();
            inbound_total.uplink = inbound_total
                .uplink
                .checked_add(reading.uplink)
                .context("inbound uplink total exceeds SQLite integer range")?;
            inbound_total.downlink = inbound_total
                .downlink
                .checked_add(reading.downlink)
                .context("inbound downlink total exceeds SQLite integer range")?;
        }

        for (user_id, username) in configured_users {
            let current = users.get(&username).copied().unwrap_or_default();
            let previous: Option<(i64, i64, Option<i64>, Option<i64>)> = tx
                .query_row(
                    "SELECT uplink_bytes,downlink_bytes,last_uplink_counter,last_downlink_counter
                     FROM proxy_user_traffic WHERE user_id=?1 AND node_id=?2",
                    params![user_id, node_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let (total_up, total_down) = match previous {
                Some((up, down, last_up, last_down)) => (
                    up.checked_add(counter_delta(current.uplink, last_up))
                        .context("user uplink total exceeds SQLite integer range")?,
                    down.checked_add(counter_delta(current.downlink, last_down))
                        .context("user downlink total exceeds SQLite integer range")?,
                ),
                None => (0, 0),
            };
            tx.execute(
                "INSERT INTO proxy_user_traffic(user_id,node_id,uplink_bytes,downlink_bytes,
                                                last_uplink_counter,last_downlink_counter,last_seen_at,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?7)
                 ON CONFLICT(user_id,node_id) DO UPDATE SET uplink_bytes=excluded.uplink_bytes,
                     downlink_bytes=excluded.downlink_bytes,last_uplink_counter=excluded.last_uplink_counter,
                     last_downlink_counter=excluded.last_downlink_counter,last_seen_at=excluded.last_seen_at,
                     updated_at=excluded.updated_at",
                params![user_id, node_id, total_up, total_down, current.uplink, current.downlink, now],
            )?;
        }

        let previous: Option<(i64, i64, Option<i64>, Option<i64>)> = tx
            .query_row(
                "SELECT uplink_bytes,downlink_bytes,last_uplink_counter,last_downlink_counter
                 FROM proxy_node_traffic WHERE node_id=?1",
                [node_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let (total_up, total_down) = match previous {
            Some((up, down, last_up, last_down)) => (
                up.checked_add(counter_delta(inbound_total.uplink, last_up))
                    .context("node uplink total exceeds SQLite integer range")?,
                down.checked_add(counter_delta(inbound_total.downlink, last_down))
                    .context("node downlink total exceeds SQLite integer range")?,
            ),
            None => (0, 0),
        };
        tx.execute(
            "INSERT INTO proxy_node_traffic(node_id,uplink_bytes,downlink_bytes,last_uplink_counter,
                                             last_downlink_counter,last_seen_at,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6,?6)
             ON CONFLICT(node_id) DO UPDATE SET uplink_bytes=excluded.uplink_bytes,
                 downlink_bytes=excluded.downlink_bytes,last_uplink_counter=excluded.last_uplink_counter,
                 last_downlink_counter=excluded.last_downlink_counter,last_seen_at=excluded.last_seen_at,
                 updated_at=excluded.updated_at",
            params![node_id, total_up, total_down, inbound_total.uplink, inbound_total.downlink, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn user_login_record(&self, username: &str) -> Result<Option<UserLoginRecord>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id,password_hash,enabled,expires_at FROM users WHERE username=?1",
                [username],
                |r| {
                    Ok(UserLoginRecord {
                        id: r.get(0)?,
                        password_hash: r.get(1)?,
                        enabled: r.get(2)?,
                        expires_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn active_user_id_for_subscription_token(&self, token: &str, now: i64) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id FROM users WHERE subscription_token=?1 AND enabled=1
               AND (expires_at IS NULL OR expires_at>?2)",
                params![token, now],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn user_subscription_token(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT subscription_token FROM users WHERE id=?1", [id], |r| r.get(0))
            .optional()?)
    }

    pub fn rotate_user_subscription_token(&self, id: i64) -> Result<Option<String>> {
        let token = random_secret();
        let changed = self.conn().execute(
            "UPDATE users SET subscription_token=?2,updated_at=?3 WHERE id=?1",
            params![id, token, Utc::now().timestamp()],
        )?;
        Ok((changed > 0).then_some(token))
    }

    pub fn reorder_user_proxies(&self, user_id: i64, ids: &[i64], now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let visible = {
            let mut stmt = tx.prepare(
                "SELECT p.id FROM user_proxy_authorizations a
                JOIN users u ON u.id=a.user_id JOIN proxies p ON p.id=a.proxy_id
                WHERE a.user_id=?1 AND a.enabled=1 AND p.enabled=1 AND u.enabled=1
                AND (u.expires_at IS NULL OR u.expires_at>?2)",
            )?;
            let rows = stmt.query_map(params![user_id, now], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<HashSet<_>>>()?
        };
        let requested: HashSet<_> = ids.iter().copied().collect();
        if ids.len() != requested.len() || requested != visible {
            refuse!("节点列表已变更或排序无效，请刷新后重试");
        }
        // Replace only this user's preference; authorization changes never mutate it.
        tx.execute("DELETE FROM user_proxy_order WHERE user_id=?1", [user_id])?;
        for (sort, id) in ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO user_proxy_order(user_id,proxy_id,sort) VALUES(?1,?2,?3)",
                params![user_id, id, sort as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn user_traffic_overview(&self, user_id: i64, reset_day: u32, now: i64) -> Result<serde_json::Value> {
        let conn = self.conn();
        let (up,down): (i64,i64) = conn.query_row(
            "SELECT COALESCE(SUM(uplink_bytes),0),COALESCE(SUM(downlink_bytes),0) FROM proxy_user_traffic WHERE user_id=?1",
            [user_id], |r| Ok((r.get(0)?,r.get(1)?)))?;
        let today =
            DateTime::from_timestamp(now, 0).context("invalid timestamp")?.with_timezone(&Local).date_naive();
        let next = if reset_day == 0 {
            None
        } else {
            (1..=32)
                .map(|days| today + chrono::Days::new(days))
                .find(|date| period_start(*date, reset_day) == *date)
        };
        Ok(serde_json::json!({
            "uplink_bytes": up, "downlink_bytes": down,
            "next_reset_date": next.map(|date| date.to_string()),
            "reset_days_remaining": next.map(|date| (date-today).num_days()),
        }))
    }

    pub fn active_user_portal_proxies(&self, id: i64, now: i64) -> Result<Vec<UserPortalProxy>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT p.id,p.node_id,n.name,p.name,p.protocol,p.address,p.address_type,p.port,
                    json_extract(p.config,'$.reality.server_name'),
                    json_extract(p.config,'$.reality.server_port'),n.group_name,n.country,
                    p.include_node_name,
                    u.uuid,json_extract(p.config,'$.reality.public_key'),
                    json_extract(p.config,'$.reality.short_id'),p.flow
             FROM user_proxy_authorizations a
             JOIN users u ON u.id=a.user_id
             JOIN proxies p ON p.id=a.proxy_id
             JOIN node n ON n.id=p.node_id
             WHERE a.user_id=?1 AND a.enabled=1 AND p.enabled=1 AND u.enabled=1
               AND (u.expires_at IS NULL OR u.expires_at>?2)
             ORDER BY (SELECT o.sort FROM user_proxy_order o WHERE o.user_id=a.user_id AND o.proxy_id=p.id) IS NULL,
                      (SELECT o.sort FROM user_proxy_order o WHERE o.user_id=a.user_id AND o.proxy_id=p.id),p.node_id,p.id",
        )?;
        let mut proxies = stmt
            .query_map(params![id, now], |r| {
                Ok(UserPortalProxy {
                    mieru_password: None,
                    id: r.get(0)?,
                    node_id: r.get(1)?,
                    node_name: r.get(2)?,
                    node_group: r.get(10)?,
                    node_country: r.get(11)?,
                    include_node_name: r.get(12)?,
                    name: r.get(3)?,
                    protocol: r.get(4)?,
                    address: r.get(5)?,
                    address_type: r.get(6)?,
                    port: r.get(7)?,
                    server_name: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    server_port: r.get::<_, Option<i64>>(9)?.unwrap_or(443),
                    uuid: r.get(13)?,
                    public_key: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                    short_id: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
                    flow: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                    online: false,
                    realm_target_node_id: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        drop(conn);
        for proxy in &mut proxies {
            if proxy.protocol == "mieru" {
                proxy.mieru_password = Some(self.mieru_password(false)?);
            }
        }
        let direct = proxies.clone();
        for entry in self.realm_subscription_entries()? {
            if let Some(base) = direct.iter().find(|p| p.id == entry.target_proxy_id) {
                let mut item = base.clone();
                item.id = -entry.id;
                item.realm_target_node_id = Some(item.node_id);
                item.node_id = entry.source_node_id;
                item.node_name = entry.source_name.clone();
                item.name = entry.name.clone();
                item.include_node_name = false;
                item.address = entry.address;
                item.address_type = if item.address.parse::<std::net::Ipv4Addr>().is_ok() {
                    "ipv4"
                } else if item.address.parse::<std::net::Ipv6Addr>().is_ok() {
                    "ipv6"
                } else {
                    "domain"
                }
                .into();
                item.port = entry.port;
                proxies.push(item);
            }
        }
        Ok(proxies)
    }

    pub fn active_user_subscription(
        &self,
        id: i64,
        now: i64,
    ) -> Result<Option<(String, Vec<UserSubscriptionProxy>)>> {
        let conn = self.conn();
        let Some((uuid, enabled, expires_at)) = conn
            .query_row("SELECT uuid,enabled,expires_at FROM users WHERE id=?1", [id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .optional()?
        else {
            return Ok(None);
        };
        if !enabled || expires_at.is_some_and(|expires| expires <= now) {
            return Ok(None);
        }
        let mut stmt = conn.prepare(
            "SELECT p.id,n.name,CASE WHEN n.country_pin='' THEN n.country ELSE n.country_pin END,
                    p.name,p.include_node_name,p.address,p.port,
                    json_extract(p.config,'$.reality.server_name'),
                    json_extract(p.config,'$.reality.public_key'),
                    json_extract(p.config,'$.reality.short_id'),
                    p.flow
             FROM user_proxy_authorizations a
             JOIN proxies p ON p.id=a.proxy_id
             JOIN node n ON n.id=p.node_id
             WHERE a.user_id=?1 AND a.enabled=1 AND p.enabled=1 AND p.protocol='vless'
             ORDER BY (SELECT o.sort FROM user_proxy_order o WHERE o.user_id=a.user_id AND o.proxy_id=p.id) IS NULL,
                      (SELECT o.sort FROM user_proxy_order o WHERE o.user_id=a.user_id AND o.proxy_id=p.id),p.node_id,p.id",
        )?;
        let mut proxies = stmt
            .query_map([id], |r| {
                Ok(UserSubscriptionProxy {
                    id: r.get(0)?,
                    node_name: r.get(1)?,
                    node_country: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    name: r.get(3)?,
                    include_node_name: r.get(4)?,
                    address: r.get(5)?,
                    port: r.get(6)?,
                    server_name: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    public_key: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    short_id: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    flow: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        drop(conn);
        let direct = proxies.clone();
        for entry in self.realm_subscription_entries()? {
            if let Some(base) = direct.iter().find(|p| p.id == entry.target_proxy_id) {
                let mut item = base.clone();
                item.id = -entry.id;
                item.name = entry.name.clone();
                item.include_node_name = false;
                item.address = entry.address;
                item.port = entry.port;
                proxies.push(item);
            }
        }
        Ok(Some((uuid, proxies)))
    }

    pub fn reset_user_uuid(&self, id: i64) -> Result<Option<(String, Vec<i64>)>> {
        let conn = self.conn();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [id], |r| r.get(0))?;
        if !exists {
            return Ok(None);
        }
        let uuid = uuid_v4();
        conn.execute(
            "UPDATE users SET uuid=?2,updated_at=?3 WHERE id=?1",
            params![id, uuid, Utc::now().timestamp()],
        )?;
        let mut stmt = conn.prepare("SELECT DISTINCT p.node_id FROM user_proxy_authorizations a JOIN proxies p ON p.id=a.proxy_id WHERE a.user_id=?1 ORDER BY p.node_id")?;
        let nodes = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some((uuid, nodes)))
    }

    pub fn nodes_for_user(&self, id: i64) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT p.node_id FROM user_proxy_authorizations a JOIN proxies p ON p.id=a.proxy_id WHERE a.user_id=?1 ORDER BY p.node_id")?;
        let rows = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn create_user(&self, draft: &UserDraft) -> Result<std::result::Result<i64, UserWriteIssue>> {
        let Some(password_hash) = draft.password_hash.as_deref() else {
            return Ok(Err(UserWriteIssue::PasswordRequired));
        };
        let conn = self.conn();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE username=?1)", [&draft.username], |r| {
                r.get(0)
            })?;
        if exists {
            return Ok(Err(UserWriteIssue::UsernameConflict));
        }
        let now = Utc::now().timestamp();
        let uuid = uuid_v4();
        conn.execute(
            "INSERT INTO users(username,uuid,password_hash,subscription_token,enabled,expires_at,created_at,updated_at,traffic_limit,device_limit,traffic_reset_day,traffic_reset_period)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?7,?8,?9,?10,?11)",
            params![draft.username, uuid, password_hash, random_secret(), draft.enabled, draft.expires_at, now,
                draft.traffic_limit.unwrap_or(0), draft.device_limit.unwrap_or(0), draft.traffic_reset_day.unwrap_or(0),
                user_reset_period(now, draft.traffic_reset_day.unwrap_or(0))?],
        )?;
        Ok(Ok(conn.last_insert_rowid()))
    }

    pub fn update_user(&self, id: i64, draft: &UserDraft) -> Result<std::result::Result<(), UserWriteIssue>> {
        self.update_user_with_uuid(id, draft, None)
    }

    pub fn update_user_with_uuid(
        &self,
        id: i64,
        draft: &UserDraft,
        uuid: Option<&str>,
    ) -> Result<std::result::Result<(), UserWriteIssue>> {
        let uuid = match uuid.map(normalize_user_uuid) {
            Some(None) => return Ok(Err(UserWriteIssue::InvalidUuid)),
            Some(Some(value)) => Some(value),
            None => None,
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let current: Option<(String, bool, Option<i64>)> = tx
            .query_row("SELECT username,enabled,expires_at FROM users WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let Some((current_username, current_enabled, current_expiry)) = current else {
            return Ok(Err(UserWriteIssue::UserNotFound));
        };
        if current_username == "admin" && draft.username != "admin" {
            return Ok(Err(UserWriteIssue::AdminUsernameLocked));
        }
        let conflict: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM users WHERE username=?1 AND id<>?2)",
            params![draft.username, id],
            |r| r.get(0),
        )?;
        if conflict {
            return Ok(Err(UserWriteIssue::UsernameConflict));
        }
        if let Some(uuid) = &uuid {
            let conflict: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM users WHERE lower(uuid)=?1 AND id<>?2)",
                params![uuid, id],
                |r| r.get(0),
            )?;
            if conflict {
                return Ok(Err(UserWriteIssue::UuidConflict));
            }
        }
        tx.execute(
            "UPDATE users SET username=?2,password_hash=COALESCE(?3,password_hash),enabled=?4,expires_at=?5,updated_at=?6,
                traffic_limit=COALESCE(?7,traffic_limit),device_limit=COALESCE(?8,device_limit),
                traffic_reset_period=CASE WHEN ?9 IS NOT NULL AND ?9<>traffic_reset_day THEN ?10 ELSE traffic_reset_period END,
                traffic_reset_day=COALESCE(?9,traffic_reset_day),uuid=COALESCE(?11,uuid) WHERE id=?1",
            params![id, draft.username, draft.password_hash, draft.enabled, draft.expires_at, Utc::now().timestamp(),
                draft.traffic_limit, draft.device_limit, draft.traffic_reset_day,
                user_reset_period(Utc::now().timestamp(), draft.traffic_reset_day.unwrap_or(0))?, uuid],
        )?;
        if draft.password_hash.is_some()
            || current_enabled != draft.enabled
            || current_expiry != draft.expires_at
        {
            tx.execute("DELETE FROM session WHERE principal_type='user' AND user_id=?1", [id])?;
        }
        tx.commit()?;
        Ok(Ok(()))
    }

    pub fn delete_user(&self, id: i64) -> Result<UserDeleteResult> {
        let conn = self.conn();
        let username: Option<String> =
            conn.query_row("SELECT username FROM users WHERE id=?1", [id], |r| r.get(0)).optional()?;
        match username.as_deref() {
            None => Ok(UserDeleteResult::NotFound),
            Some("admin") => Ok(UserDeleteResult::Protected),
            Some(_) => {
                conn.execute("DELETE FROM session WHERE principal_type='user' AND user_id=?1", [id])?;
                conn.execute("DELETE FROM users WHERE id=?1", [id])?;
                Ok(UserDeleteResult::Deleted)
            }
        }
    }

    pub fn authorizations_for_user(&self, user_id: i64) -> Result<Option<Vec<UserProxyAuthorizationItem>>> {
        let conn = self.conn();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user_id], |r| r.get(0))?;
        if !exists {
            return Ok(None);
        }
        let mut stmt = conn.prepare(
            "SELECT p.id,p.node_id,p.name,p.protocol,p.address_type,p.address,p.port,p.enabled,
                    p.include_node_name,
                    a.enabled,a.auth
             FROM user_proxy_authorizations a JOIN proxies p ON p.id=a.proxy_id
             WHERE a.user_id=?1 ORDER BY p.id",
        )?;
        let rows = stmt
            .query_map([user_id], |r| {
                Ok(UserProxyAuthorizationItem {
                    proxy: ProxySummary {
                        id: r.get(0)?,
                        node_id: r.get(1)?,
                        name: r.get(2)?,
                        protocol: r.get(3)?,
                        address_type: r.get(4)?,
                        address: r.get(5)?,
                        port: r.get(6)?,
                        enabled: r.get(7)?,
                        include_node_name: r.get(8)?,
                    },
                    access: AuthorizationSummary { enabled: r.get(9)?, auth: parse_json(r, 10)? },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(rows))
    }

    pub fn put_authorization(
        &self,
        user_id: i64,
        proxy_id: i64,
        draft: &AuthorizationDraft,
    ) -> Result<std::result::Result<AuthorizationRecord, AuthorizationWriteIssue>> {
        let auth = draft.auth.as_ref().map(serde_json::to_string).transpose()?.unwrap_or_else(|| "{}".into());
        let conn = self.conn();
        let user_exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user_id], |r| r.get(0))?;
        if !user_exists {
            return Ok(Err(AuthorizationWriteIssue::UserNotFound));
        }
        let proxy_exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM proxies WHERE id=?1)", [proxy_id], |r| r.get(0))?;
        if !proxy_exists {
            return Ok(Err(AuthorizationWriteIssue::ProxyNotFound));
        }
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT INTO user_proxy_authorizations(user_id,proxy_id,enabled,auth,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?5)
             ON CONFLICT(user_id,proxy_id) DO UPDATE SET enabled=excluded.enabled,
                 auth=CASE WHEN ?6 THEN excluded.auth ELSE user_proxy_authorizations.auth END,
                 updated_at=excluded.updated_at",
            params![user_id, proxy_id, draft.enabled, auth, now, draft.auth.is_some()],
        )?;
        let access = conn.query_row(
            "SELECT id,user_id,proxy_id,enabled,auth,created_at,updated_at
             FROM user_proxy_authorizations WHERE user_id=?1 AND proxy_id=?2",
            params![user_id, proxy_id],
            |r| {
                Ok(AuthorizationRecord {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    proxy_id: r.get(2)?,
                    enabled: r.get(3)?,
                    auth: parse_json(r, 4)?,
                    created_at: r.get(5)?,
                    updated_at: r.get(6)?,
                })
            },
        )?;
        Ok(Ok(access))
    }

    pub fn delete_authorization(&self, user_id: i64, proxy_id: i64) -> Result<bool> {
        Ok(self.conn().execute(
            "DELETE FROM user_proxy_authorizations WHERE user_id=?1 AND proxy_id=?2",
            params![user_id, proxy_id],
        )? > 0)
    }

    /// Replaces all authorizations for one user atomically and returns the
    /// distinct nodes whose generated configuration may have changed.
    pub fn replace_authorizations(
        &self,
        user_id: i64,
        items: &[(i64, bool)],
    ) -> Result<std::result::Result<Vec<i64>, AuthorizationWriteIssue>> {
        self.replace_authorizations_with_policy(user_id, items, None)
    }

    pub fn replace_authorizations_with_policy(
        &self,
        user_id: i64,
        items: &[(i64, bool)],
        auto_authorize_new_proxies: Option<bool>,
    ) -> Result<std::result::Result<Vec<i64>, AuthorizationWriteIssue>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let user_exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user_id], |r| r.get(0))?;
        if !user_exists {
            return Ok(Err(AuthorizationWriteIssue::UserNotFound));
        }

        let mut node_ids = std::collections::BTreeSet::new();
        {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT p.node_id FROM user_proxy_authorizations a
                 JOIN proxies p ON p.id=a.proxy_id WHERE a.user_id=?1",
            )?;
            let rows = stmt.query_map([user_id], |r| r.get::<_, i64>(0))?;
            node_ids.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        for (proxy_id, _) in items {
            let node_id: Option<i64> = tx
                .query_row("SELECT node_id FROM proxies WHERE id=?1", [proxy_id], |r| r.get(0))
                .optional()?;
            let Some(node_id) = node_id else {
                return Ok(Err(AuthorizationWriteIssue::ProxyNotFound));
            };
            node_ids.insert(node_id);
        }

        tx.execute("DELETE FROM user_proxy_authorizations WHERE user_id=?1", [user_id])?;
        let now = Utc::now().timestamp();
        for (proxy_id, enabled) in items {
            tx.execute(
                "INSERT INTO user_proxy_authorizations(user_id,proxy_id,enabled,auth,created_at,updated_at)
                 VALUES(?1,?2,?3,'{}',?4,?4)",
                params![user_id, proxy_id, enabled, now],
            )?;
        }
        if let Some(enabled) = auto_authorize_new_proxies {
            tx.execute(
                "UPDATE users SET auto_authorize_new_proxies=?2,updated_at=?3 WHERE id=?1",
                params![user_id, enabled, now],
            )?;
        }
        tx.commit()?;
        Ok(Ok(node_ids.into_iter().collect()))
    }

    /// Returns expired users still authorized on this node. The caller invokes
    /// this only after a traffic counter changes, avoiding a periodic user scan.
    pub fn expired_authorized_usernames_for_node(
        &self,
        node_id: i64,
        now: i64,
    ) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT u.username FROM users u
             JOIN user_proxy_authorizations a ON a.user_id=u.id
             JOIN proxies p ON p.id=a.proxy_id
             WHERE p.node_id=?1 AND p.protocol='vless' AND p.enabled=1 AND a.enabled=1 AND u.enabled=1
               AND u.expires_at IS NOT NULL AND u.expires_at<=?2",
        )?;
        let rows = stmt.query_map(params![node_id, now], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?)
    }

    /// Only active authorizations are materialized into a generated inbound.
    pub fn active_users_for_proxy(&self, proxy_id: i64, now: i64) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT u.username,u.uuid,a.auth FROM user_proxy_authorizations a JOIN users u ON u.id=a.user_id
             WHERE a.proxy_id=?1 AND a.enabled=1 AND u.enabled=1
               AND (u.expires_at IS NULL OR u.expires_at>?2)
             ORDER BY a.id",
        )?;
        let rows = stmt
            .query_map(params![proxy_id, now], |r| {
                let username: String = r.get(0)?;
                let uuid: String = r.get(1)?;
                let mut auth = parse_json(r, 2)?;
                if let Some(object) = auth.as_object_mut() {
                    object.insert("name".into(), username.into());
                    object.insert("uuid".into(), uuid.into());
                }
                Ok(auth)
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Creates a node and returns its id.
    ///
    /// One transaction: `accumulate` reads the `traffic` row on every report, so
    /// a node lacking one cannot report. The node also takes every `auto_join`
    /// probe here, at most [`Self::MAX_PROBES_PER_NODE`] rows, a limit
    /// `save_ping_task` enforces.
    pub fn create_node(&self, n: &Node, token: &str) -> Result<i64> {
        self.create_node_with_components(n, token, &InstallChoices::default())
    }
    pub fn create_node_with_components(
        &self,
        n: &Node,
        token: &str,
        choices: &InstallChoices,
    ) -> Result<i64> {
        choices.validate()?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let id = next_id(&tx, "node")?;
        tx.execute(
            // A new node belongs at the end. The caller sends sort 0, which would
            // tie with whatever the last reorder placed first.
            "INSERT INTO node (id, name, token, sort, public, price, currency, billing_cycle,
                               expires_at, remark, traffic_limit, traffic_mode, traffic_reset_day, created_at,
                               group_name)
             VALUES (?14,?1,?2,(SELECT COALESCE(MAX(sort),-1)+1 FROM node),?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
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
                n.group,
                id
            ],
        )?;
        // Only a registration without `--name` arrives unnamed; the panel
        // requires a name.
        if n.name.is_empty() {
            tx.execute("UPDATE node SET name = ?1 WHERE id = ?2", params![format!("节点 {id}"), id])?;
        }
        seed_components(&tx, id, choices)?;
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
                "UPDATE node SET name=COALESCE(?2,name), public=COALESCE(?3,public),
                                 price=COALESCE(?4,price), currency=COALESCE(?5,currency),
                                 billing_cycle=COALESCE(?6,billing_cycle),
                                 expires_at=CASE WHEN ?7 THEN ?8 ELSE expires_at END,
                                 remark=COALESCE(?9,remark), traffic_limit=COALESCE(?10,traffic_limit),
                                 traffic_mode=COALESCE(?11,traffic_mode),
                                 traffic_reset_day=COALESCE(?12,traffic_reset_day),
                                 notify=COALESCE(?13,notify), country_pin=COALESCE(?14,country_pin),
                                 ipv4_pin=COALESCE(?15,ipv4_pin), ipv6_pin=COALESCE(?16,ipv6_pin),
                                 group_name=COALESCE(?17,group_name),
                                 public_remark=COALESCE(?18,public_remark)
                 WHERE id=?1",
            )?;
            for id in ids {
                let found = update.execute(params![
                    id,
                    n.name,
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
                    n.group,
                    n.public_remark
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

    pub fn reorder_proxies(&self, ids: &[i64]) -> Result<()> {
        self.reorder("proxies", ids)
    }

    pub fn reorder_ping_tasks(&self, ids: &[i64]) -> Result<()> {
        self.reorder("ping_task", ids)
    }

    /// Renumbers `sort` from a list that must name every row exactly once, so a
    /// tab that missed an insert or a delete cannot renumber around it.
    pub(super) fn reorder(&self, table: &str, ids: &[i64]) -> Result<()> {
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
        // for the chart query -- so it is cleared explicitly; the other history
        // tables cascade.
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
        // Six of these -- os, kernel, arch, virt, cpu_name, agent_version -- go
        // straight into the anonymous public frame, which is rebuilt and pushed to
        // every viewer every two seconds, so without a ceiling one node would
        // determine that frame's size. 128 rather than 64: a real PRETTY_NAME runs
        // to about 60 characters and a CPU model to about 50.
        let s = |k: &str| reported_text(f.get(k).and_then(|v| v.as_str()).unwrap_or(""), 128);
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
}

impl Db {
    /// Records where the hub saw a node over the address family its session is
    /// not in. Kept apart from `save_facts`, which an agent's hello rewrites: the
    /// two arrive on different connections, in no fixed order.
    pub fn set_ip_other(&self, id: i64, ip: &str) -> Result<()> {
        // Unchanged is not written: a reconnecting agent, or a token holder
        // repeating the request, would otherwise cost a write transaction each.
        self.conn().execute("UPDATE node SET ip_other=?2 WHERE id=?1 AND ip_other<>?2", params![id, ip])?;
        Ok(())
    }
}
