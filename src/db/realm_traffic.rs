use super::*;
use serde_json::{json, Value};
pub(super) const REALM_TRAFFIC_TABLES: [&str; 3] =
    ["realm_traffic_rules", "realm_traffic_buckets", "realm_traffic_status"];
pub(super) const REALM_TRAFFIC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS realm_traffic_rules(key TEXT PRIMARY KEY,node_id INTEGER NOT NULL,rule_id INTEGER NOT NULL,name TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS realm_traffic_rules_node ON realm_traffic_rules(node_id);
CREATE TABLE IF NOT EXISTS realm_traffic_buckets(node_id INTEGER NOT NULL,collector TEXT NOT NULL,key TEXT NOT NULL,minute INTEGER NOT NULL,up INTEGER NOT NULL,down INTEGER NOT NULL,revision INTEGER NOT NULL,sample INTEGER NOT NULL,up_rate INTEGER,down_rate INTEGER,PRIMARY KEY(node_id,collector,key,minute));
CREATE INDEX IF NOT EXISTS realm_traffic_buckets_rule ON realm_traffic_buckets(key,minute);
CREATE INDEX IF NOT EXISTS realm_traffic_buckets_sample ON realm_traffic_buckets(sample);
CREATE TABLE IF NOT EXISTS realm_traffic_status(node_id INTEGER PRIMARY KEY,status TEXT NOT NULL,received INTEGER NOT NULL);
"#;
fn valid_key(v: &str) -> bool {
    v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit())
}
impl Db {
    pub fn realm_traffic_recheck(&self, node: i64) -> Result<()> {
        let n=self.conn().execute("UPDATE realm_hosts SET traffic_recheck=lower(hex(randomblob(16))),request_nonce=request_nonce+1 WHERE node_id=?1",[node])?;
        anyhow::ensure!(n > 0, "Realm host missing");
        Ok(())
    }
    pub fn realm_traffic_record(&self, node: i64, v: &Value) -> Result<Value> {
        anyhow::ensure!(v.to_string().len() <= 48 * 1024, "traffic frame too large");
        let collector = v["collector"].as_str().unwrap_or("");
        anyhow::ensure!(valid_key(collector), "invalid collector");
        let records = v["records"].as_array().context("missing records")?;
        anyhow::ensure!(records.len() <= 128, "too many traffic records");
        let status = &v["status"];
        anyhow::ensure!(
            status.to_string().len() <= 2048
                && ["waiting", "monitoring", "unmonitored"].contains(&status["state"].as_str().unwrap_or("")),
            "invalid traffic status"
        );
        let ts = Utc::now().timestamp();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut ack = Vec::new();
        let mut discarded = 0_i64;
        for r in records {
            let key = r["key"].as_str().unwrap_or("");
            anyhow::ensure!(valid_key(key), "invalid traffic key");
            let integer = |k: &str| r[k].as_i64().with_context(|| format!("invalid {k}"));
            let (minute, up, down, rev, id, sample) = (
                integer("minute")?,
                integer("up")?,
                integer("down")?,
                integer("revision")?,
                integer("id")?,
                integer("sample")?,
            );
            anyhow::ensure!(
                minute % 60 == 0
                    && minute >= ts - 8 * 86400
                    && minute <= ts + 300
                    && up >= 0
                    && down >= 0
                    && up <= i64::MAX / 4096
                    && down <= i64::MAX / 4096
                    && rev > 0
                    && id > 0
                    && sample <= ts + 300,
                "invalid traffic bucket"
            );
            let owner: Option<i64> = tx
                .query_row("SELECT node_id FROM realm_traffic_rules WHERE key=?1", [key], |r| r.get(0))
                .optional()?;
            if let Some(owner) = owner {
                anyhow::ensure!(owner == node, "traffic rule belongs to another node");
            } else {
                // A restored backup can lack rules still queued on an Agent. Drain those
                // records without inventing attribution or blocking all future reports.
                discarded += 1;
                ack.push(json!({"id":id,"revision":rev}));
                continue;
            }
            // Cumulative minute upsert: retransmission, stale ACK and reconnect cannot double count.
            tx.execute("INSERT INTO realm_traffic_buckets VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(node_id,collector,key,minute) DO UPDATE SET up=excluded.up,down=excluded.down,revision=excluded.revision,sample=excluded.sample,up_rate=excluded.up_rate,down_rate=excluded.down_rate WHERE excluded.revision>realm_traffic_buckets.revision AND excluded.up>=realm_traffic_buckets.up AND excluded.down>=realm_traffic_buckets.down",params![node,collector,key,minute,up,down,rev,sample,r["up_rate"].as_i64().filter(|x|*x>=0),r["down_rate"].as_i64().filter(|x|*x>=0)])?;
            ack.push(json!({"id":id,"revision":rev}));
        }
        let previous: Option<String> = tx
            .query_row("SELECT status FROM realm_traffic_status WHERE node_id=?1", [node], |r| r.get(0))
            .optional()?;
        let previous = previous
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["discarded_records"].as_i64())
            .unwrap_or(0);
        let mut status = status.clone();
        status["discarded_records"] = json!(previous.saturating_add(discarded));
        tx.execute("INSERT INTO realm_traffic_status VALUES(?1,?2,?3) ON CONFLICT(node_id) DO UPDATE SET status=excluded.status,received=excluded.received",params![node,status.to_string(),ts])?;
        tx.commit()?;
        Ok(json!({"type":"realm_traffic_ack","collector":collector,"records":ack}))
    }
    pub fn realm_traffic(&self) -> Result<Value> {
        let ts = Utc::now().timestamp();
        let day = ts.div_euclid(86400) * 86400;
        let month =
            Utc::now().date_naive().with_day(1).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
        let conn = self.conn();
        let names = realm_display_names(&conn)?;
        let mut stmt=conn.prepare("SELECT r.key,r.node_id,r.rule_id,r.name,EXISTS(SELECT 1 FROM realm_rules live WHERE live.traffic_key=r.key),COALESCE(SUM(b.up),0),COALESCE(SUM(b.down),0),COALESCE(SUM(CASE WHEN b.minute>=?1 THEN b.up+b.down ELSE 0 END),0),COALESCE(SUM(CASE WHEN b.minute>=?2 THEN b.up+b.down ELSE 0 END),0),MAX(b.sample) FROM realm_traffic_rules r LEFT JOIN realm_traffic_buckets b ON b.key=r.key GROUP BY r.key ORDER BY r.node_id,r.rule_id")?;
        let mut rules=stmt.query_map(params![day,month],|r|Ok(json!({"key":r.get::<_,String>(0)?,"node_id":r.get::<_,i64>(1)?,"rule_id":r.get::<_,i64>(2)?,"name":names.get(&r.get::<_,i64>(2)?).cloned().unwrap_or(realm_name(&r.get::<_,String>(3)?)),"active":r.get::<_,bool>(4)?,"up":r.get::<_,i64>(5)?,"down":r.get::<_,i64>(6)?,"today":r.get::<_,i64>(7)?,"month":r.get::<_,i64>(8)?,"last_sample":r.get::<_,Option<i64>>(9)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt = conn.prepare("SELECT node_id,status,received FROM realm_traffic_status")?;
        let hosts = stmt
            .query_map([], |r| {
                let mut status: Value = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or(json!({}));
                let received = r.get::<_, i64>(2)?;
                let sample = status["last_sample"].as_i64().unwrap_or(0);
                let fresh = received >= ts - 20 && sample >= ts - 20 && status["state"] == "monitoring";
                status["fresh"] = json!(fresh);
                Ok(json!({"node_id":r.get::<_,i64>(0)?,"status":status,"received":received}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rate_stmt=conn.prepare("SELECT key,up_rate,down_rate,MAX(sample) FROM realm_traffic_buckets WHERE sample>=?1 GROUP BY key")?;
        let rates = rate_stmt
            .query_map([ts - 20], |r| {
                Ok((r.get::<_, String>(0)?, (r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?)))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        for rule in &mut rules {
            let key = rule["key"].as_str().unwrap_or("");
            let fresh = hosts.iter().any(|h| h["node_id"] == rule["node_id"] && h["status"]["fresh"] == true);
            let (up, down) = if fresh { rates.get(key).copied().unwrap_or_default() } else { (None, None) };
            rule["up_rate"] = json!(up);
            rule["down_rate"] = json!(down);
        }
        Ok(json!({"hosts":hosts,"rules":rules,"timezone":"UTC"}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retransmission_is_idempotent_and_stale_records_cannot_decrease_usage() {
        let db = Db::open(":memory:").unwrap();
        let key = "a".repeat(32);
        db.conn().execute("INSERT INTO realm_traffic_rules VALUES(?1,1,1,'old rule')", [&key]).unwrap();
        let mut v = json!({"collector":"b".repeat(32),"status":{"state":"monitoring","last_sample":Utc::now().timestamp()},"records":[{"id":1,"key":key,"minute":Utc::now().timestamp()/60*60,"up":100,"down":200,"revision":1,"sample":Utc::now().timestamp()}]});
        db.realm_traffic_record(1, &v).unwrap();
        db.realm_traffic_record(1, &v).unwrap();
        assert_eq!(db.realm_traffic().unwrap()["rules"][0]["up"], 100);
        v["records"][0]["revision"] = json!(2);
        v["records"][0]["up"] = json!(150);
        db.realm_traffic_record(1, &v).unwrap();
        assert_eq!(db.realm_traffic().unwrap()["rules"][0]["up"], 150);
        v["records"][0]["up"] = json!(1);
        v["records"][0]["revision"] = json!(3);
        db.realm_traffic_record(1, &v).unwrap();
        assert_eq!(db.realm_traffic().unwrap()["rules"][0]["up"], 150);
        assert!(db.realm_traffic_record(2, &v).is_err());
        db.conn().execute("DELETE FROM realm_traffic_rules", []).unwrap();
        assert_eq!(db.realm_traffic_record(1, &v).unwrap()["records"].as_array().unwrap().len(), 1);
        assert_eq!(db.realm_traffic().unwrap()["hosts"][0]["status"]["discarded_records"], 1);
        assert!(db.realm_traffic().unwrap()["rules"].as_array().unwrap().is_empty());
    }
    #[test]
    fn delayed_minutes_keep_their_original_day_and_are_not_live_rates() {
        let db = Db::open(":memory:").unwrap();
        let key = "c".repeat(32);
        db.conn().execute("INSERT INTO realm_traffic_rules VALUES(?1,1,1,'deleted')", [&key]).unwrap();
        let now = Utc::now().timestamp();
        let minute = (now / 86400 * 86400) - 60;
        let v = json!({"collector":"d".repeat(32),"status":{"state":"unmonitored","last_sample":minute},"records":[{"id":1,"key":key,"minute":minute,"up":10,"down":20,"revision":1,"sample":minute,"up_rate":10,"down_rate":20}]});
        db.realm_traffic_record(1, &v).unwrap();
        let view = db.realm_traffic().unwrap();
        let rule = &view["rules"][0];
        assert_eq!(rule["up"], 10);
        assert_eq!(rule["today"], 0);
        assert!(rule["up_rate"].is_null());
        assert_eq!(view["hosts"][0]["status"]["fresh"], false);
        let mut large = v;
        large["records"] = json!(vec![large["records"][0].clone(); 129]);
        assert!(db.realm_traffic_record(1, &large).is_err());
    }
}
