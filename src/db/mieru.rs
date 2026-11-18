use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) const MIERU_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS user_proxy_credentials(user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,protocol TEXT NOT NULL,password TEXT NOT NULL,PRIMARY KEY(user_id,protocol));
CREATE TABLE IF NOT EXISTS mieru_hosts(node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE CASCADE,revision INTEGER NOT NULL DEFAULT 0,digest TEXT NOT NULL DEFAULT '',applied_revision INTEGER NOT NULL DEFAULT 0,status TEXT NOT NULL DEFAULT '{}',last_error TEXT NOT NULL DEFAULT '',traffic_error TEXT NOT NULL DEFAULT '',traffic_received INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS mieru_traffic_sources(node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,identity TEXT NOT NULL,epoch TEXT NOT NULL,seq INTEGER NOT NULL,up INTEGER NOT NULL,down INTEGER NOT NULL,PRIMARY KEY(node_id,identity));
"#;
#[derive(Clone)]
pub struct MieruDesired {
    pub node_id: i64,
    pub revision: i64,
    pub digest: String,
    pub content: String,
}
fn shared_mieru_password(conn: &Connection) -> Result<String> {
    if let Some(password) = conn
        .query_row("SELECT value FROM setting WHERE key='mieru_shared_password'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
    {
        return Ok(password);
    }
    conn.execute(
        "INSERT OR IGNORE INTO setting(key,value) VALUES('mieru_shared_password',?1)",
        [random_secret()],
    )?;
    Ok(conn.query_row("SELECT value FROM setting WHERE key='mieru_shared_password'", [], |r| r.get(0))?)
}

impl Db {
    pub fn mieru_password(&self, reset: bool) -> Result<String> {
        let conn = self.conn();
        if reset {
            put_setting(&conn, "mieru_shared_password", &random_secret())?;
        }
        shared_mieru_password(&conn)
    }
    pub fn mieru_managed(&self, node: i64) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM mieru_hosts WHERE node_id=?1)",
            [node],
            |r| r.get(0),
        )?)
    }
    pub fn mieru_hosts(&self) -> Result<Vec<Value>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node_id,revision,applied_revision,status,last_error,traffic_error,traffic_received FROM mieru_hosts ORDER BY node_id",
        )?;
        let rows=stmt.query_map([],|r|Ok(json!({"node_id":r.get::<_,i64>(0)?,"revision":r.get::<_,i64>(1)?,"applied_revision":r.get::<_,i64>(2)?,"status":serde_json::from_str::<Value>(&r.get::<_,String>(3)?).unwrap_or(json!({})),"last_error":r.get::<_,String>(4)?,"traffic_error":r.get::<_,String>(5)?,"traffic_received":r.get::<_,i64>(6)?})))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    /// Snapshot authorizations, create credentials and advance revision under one writer transaction.
    /// An older generator can never publish a newer revision after a concurrent revocation.
    pub fn mieru_desired(&self, node: i64) -> Result<Option<MieruDesired>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let proxy: Option<(i64, bool, i64)> = tx
            .query_row(
                "SELECT id,enabled,port FROM proxies WHERE node_id=?1 AND protocol='mieru'",
                [node],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let managed: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM mieru_hosts WHERE node_id=?1)", [node], |r| r.get(0))?;
        if proxy.is_none() && !managed {
            return Ok(None);
        }
        let mut users = Vec::new();
        let mut port = 0;
        let mut proxy_id = 0;
        if let Some((id, true, p)) = proxy {
            port = p;
            proxy_id = id;
            let ids = {
                let mut stmt=tx.prepare("SELECT u.id FROM users u JOIN user_proxy_authorizations a ON a.user_id=u.id WHERE a.proxy_id=?1 AND a.enabled=1 AND u.enabled=1 AND (u.expires_at IS NULL OR u.expires_at>?2) ORDER BY u.id")?;
                let rows = stmt
                    .query_map(params![id, Utc::now().timestamp()], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            anyhow::ensure!(ids.len() <= 4096, "mieru supports at most 4096 users per node");
            let password = if ids.is_empty() { String::new() } else { shared_mieru_password(&tx)? };
            for user in ids {
                users.push(json!({"name":format!("user-{user}"),"password":password}));
            }
        }
        let run = port > 0 && !users.is_empty();
        let config = json!({"portBindings":if port>0{json!([{"port":port,"protocol":"TCP"}])}else{json!([])},"users":users,"loggingLevel":"WARN","mtu":1400,"dns":{"dualStack":"PREFER_IPv4"}});
        let content = json!({"proxy_id":proxy_id,"run":run,"config":config}).to_string();
        anyhow::ensure!(content.len() <= 1024 * 1024, "mieru config exceeds 1 MiB");
        let digest = hex::encode(Sha256::digest(content.as_bytes()));
        tx.execute("INSERT INTO mieru_hosts(node_id,revision,digest) VALUES(?1,1,?2) ON CONFLICT(node_id) DO UPDATE SET revision=revision+CASE WHEN digest<>excluded.digest THEN 1 ELSE 0 END,digest=excluded.digest",params![node,digest])?;
        let revision =
            tx.query_row("SELECT revision FROM mieru_hosts WHERE node_id=?1", [node], |r| r.get(0))?;
        tx.commit()?;
        Ok(Some(MieruDesired { node_id: node, revision, digest, content }))
    }
    pub fn mieru_rebase(&self, node: i64, revision: i64) -> Result<()> {
        let next = revision.checked_add(1).context("mieru revision exhausted")?;
        self.conn().execute(
            "UPDATE mieru_hosts SET revision=MAX(revision,?2) WHERE node_id=?1",
            params![node, next],
        )?;
        Ok(())
    }
    pub fn mieru_observed(&self, node: i64, status: &Value, error: &str) -> Result<()> {
        self.conn().execute("UPDATE mieru_hosts SET status=?2,last_error=?3,applied_revision=CASE WHEN json_extract(?2,'$.digest')=digest AND json_extract(?2,'$.revision')=revision THEN revision ELSE applied_revision END WHERE node_id=?1",params![node,status.to_string(),error])?;
        Ok(())
    }
    pub fn record_mieru_traffic(&self, node: i64, v: &Value, now: i64) -> Result<()> {
        let epoch = v["epoch"].as_str().context("missing mieru epoch")?;
        let seq = v["seq"].as_i64().filter(|n| *n >= 0).context("invalid sequence")?;
        anyhow::ensure!(!epoch.is_empty() && epoch.len() <= 128, "invalid epoch");
        if let Some(error) = v["error"].as_str() {
            self.conn().execute(
                "UPDATE mieru_hosts SET traffic_error=?2,traffic_received=?3 WHERE node_id=?1",
                params![node, error.chars().take(256).collect::<String>(), now],
            )?;
            return Ok(());
        }
        let users = v["users"].as_object().context("missing users")?;
        anyhow::ensure!(users.len() <= 4096, "too many mieru users");
        let proxy = v["proxy_id"].as_i64().context("missing proxy id")?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let valid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxies WHERE id=?1 AND node_id=?2 AND protocol='mieru')",
            params![proxy, node],
            |r| r.get(0),
        )?;
        if !valid {
            return Ok(());
        }
        reset_due_user_traffic(&tx, now)?;
        for (identity, counters) in
            users.iter().map(|(k, v)| (k.as_str(), v)).chain(std::iter::once(("@node", &v["total"])))
        {
            let up = counters["uplink"].as_i64().filter(|n| *n >= 0).context("invalid mieru uplink")?;
            let down = counters["downlink"].as_i64().filter(|n| *n >= 0).context("invalid mieru downlink")?;
            let user = if identity == "@node" {
                None
            } else {
                let id = identity
                    .strip_prefix("user-")
                    .and_then(|s| s.parse::<i64>().ok())
                    .context("invalid mieru identity")?;
                if !tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [id], |r| {
                    r.get::<_, bool>(0)
                })? {
                    continue;
                }
                Some(id)
            };
            let key = format!("{proxy}:{identity}");
            let previous: Option<(String, i64, i64, i64)> = tx
                .query_row(
                    "SELECT epoch,seq,up,down FROM mieru_traffic_sources WHERE node_id=?1 AND identity=?2",
                    params![node, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if previous.as_ref().is_some_and(|p| p.0 == epoch && p.1 >= seq) {
                continue;
            }
            let (du, dd) = match previous {
                Some((e, _, u, d)) if e == epoch && up >= u && down >= d => (up - u, down - d),
                _ => (0, 0),
            };
            tx.execute("INSERT INTO mieru_traffic_sources VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(node_id,identity) DO UPDATE SET epoch=excluded.epoch,seq=excluded.seq,up=excluded.up,down=excluded.down",params![node,key,epoch,seq,up,down])?;
            if let Some(user) = user {
                let (u,d):(i64,i64)=tx.query_row("SELECT uplink_bytes,downlink_bytes FROM proxy_user_traffic WHERE user_id=?1 AND node_id=?2",params![user,node],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.unwrap_or((0,0));
                anyhow::ensure!(
                    u.checked_add(du).is_some() && d.checked_add(dd).is_some(),
                    "mieru traffic overflow"
                );
                tx.execute("INSERT INTO proxy_user_traffic(user_id,node_id,uplink_bytes,downlink_bytes,last_seen_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5,?5) ON CONFLICT(user_id,node_id) DO UPDATE SET uplink_bytes=uplink_bytes+excluded.uplink_bytes,downlink_bytes=downlink_bytes+excluded.downlink_bytes,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",params![user,node,du,dd,now])?;
            } else {
                let (u, d): (i64, i64) = tx
                    .query_row(
                        "SELECT uplink_bytes,downlink_bytes FROM proxy_node_traffic WHERE node_id=?1",
                        [node],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?
                    .unwrap_or((0, 0));
                anyhow::ensure!(
                    u.checked_add(du).is_some() && d.checked_add(dd).is_some(),
                    "mieru traffic overflow"
                );
                tx.execute("INSERT INTO proxy_node_traffic(node_id,uplink_bytes,downlink_bytes,last_seen_at,created_at,updated_at) VALUES(?1,?2,?3,?4,?4,?4) ON CONFLICT(node_id) DO UPDATE SET uplink_bytes=uplink_bytes+excluded.uplink_bytes,downlink_bytes=downlink_bytes+excluded.downlink_bytes,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",params![node,du,dd,now])?;
            }
        }
        tx.execute(
            "UPDATE mieru_hosts SET traffic_error='',traffic_received=?2 WHERE node_id=?1",
            params![node, now],
        )?;
        tx.commit()?;
        Ok(())
    }
}
