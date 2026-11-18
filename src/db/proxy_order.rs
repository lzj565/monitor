use super::*;

pub(super) const PROXY_LIST_ORDER_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS proxy_list_order (key TEXT PRIMARY KEY, position INTEGER NOT NULL);";

fn live_keys(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT 'proxy:' || id FROM proxies ORDER BY sort,id")?;
    let keys = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(keys)
}

impl Db {
    pub fn proxy_list_order(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let live = live_keys(&conn)?;
        let mut stmt = conn.prepare("SELECT key FROM proxy_list_order ORDER BY position,key")?;
        let mut saved =
            stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let valid: HashSet<_> = live.iter().collect();
        saved.retain(|key| valid.contains(key));
        let seen: HashSet<_> = saved.iter().cloned().collect();
        saved.extend(live.into_iter().filter(|key| !seen.contains(key)));
        Ok(saved)
    }

    pub fn save_proxy_list_order(&self, ids: &[String]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let live: HashSet<_> = live_keys(&tx)?.into_iter().collect();
        let supplied: HashSet<_> = ids.iter().cloned().collect();
        if ids.len() != supplied.len() || live != supplied {
            refuse!("列表已在别处改动或排序包含重复条目，刷新后再排序");
        }
        tx.execute("DELETE FROM proxy_list_order", [])?;
        for (position, key) in ids.iter().enumerate() {
            tx.execute(
                "INSERT INTO proxy_list_order(key,position) VALUES(?1,?2)",
                params![key, position as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
