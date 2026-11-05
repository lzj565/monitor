//! External proxy storage is deliberately independent of monitored nodes.

pub(super) const EXTERNAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS external_proxies (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL UNIQUE,
 protocol TEXT NOT NULL,
 address TEXT NOT NULL,
 port INTEGER NOT NULL CHECK(port BETWEEN 1 AND 65535),
 config TEXT NOT NULL,
 enabled INTEGER NOT NULL DEFAULT 1,
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS user_external_proxy_authorizations (
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 external_proxy_id INTEGER NOT NULL REFERENCES external_proxies(id) ON DELETE CASCADE,
 enabled INTEGER NOT NULL DEFAULT 1,
 PRIMARY KEY(user_id,external_proxy_id)
);
"#;

#[cfg(test)]
mod tests {
    use crate::db::Db;
    #[test]
    fn external_proxy_storage_requires_no_node() {
        let db = Db::open(":memory:").unwrap();
        let conn = db.conn();
        conn.execute("INSERT INTO external_proxies(name,protocol,address,port,config,created_at,updated_at) VALUES('external','trojan','example.com',443,'{}',0,0)",[]).unwrap();
        let id = conn.last_insert_rowid();
        assert_eq!(conn.query_row("SELECT count(*) FROM node", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        conn.execute("DELETE FROM external_proxies WHERE id=?1", [id]).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM external_proxies", [], |r| r.get::<_, i64>(0)).unwrap(),
            0
        );
    }
}

use super::*;

#[derive(serde::Serialize)]
pub struct ExternalProxy {
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub address: String,
    pub port: i64,
    pub enabled: bool,
}

impl Db {
    pub fn external_proxies(&self) -> Result<Vec<ExternalProxy>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id,name,protocol,address,port,enabled FROM external_proxies ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ExternalProxy {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    protocol: r.get(2)?,
                    address: r.get(3)?,
                    port: r.get(4)?,
                    enabled: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn import_external_proxies(&self, entries: &[serde_json::Value]) -> Result<Vec<i64>> {
        if entries.is_empty() || entries.len() > 500 {
            refuse!("一次须导入1至500条代理");
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut ids = Vec::new();
        for entry in entries {
            let config = crate::external_proxy::validate(entry)?;
            let now = Utc::now().timestamp();
            tx.execute("INSERT INTO external_proxies(name,protocol,address,port,config,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",params![config["name"].as_str(),config["type"].as_str(),config["server"].as_str(),config["port"].as_i64(),config.to_string(),now])?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn external_proxy_config(&self, id: i64) -> Result<Option<serde_json::Value>> {
        let text: Option<String> = self
            .conn()
            .query_row("SELECT config FROM external_proxies WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        text.map(|s| serde_json::from_str(&s).map_err(Into::into)).transpose()
    }

    pub fn replace_external_authorizations(&self, user: i64, ids: &[i64]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user], |r| r.get(0))?;
        if !exists {
            refuse!("用户不存在");
        }
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id) {
                refuse!("重复的外部代理");
            }
            let exists: bool =
                tx.query_row("SELECT EXISTS(SELECT 1 FROM external_proxies WHERE id=?1)", [id], |r| {
                    r.get(0)
                })?;
            if !exists {
                refuse!("外部代理不存在");
            }
        }
        tx.execute("DELETE FROM user_external_proxy_authorizations WHERE user_id=?1", [user])?;
        for id in ids {
            tx.execute(
                "INSERT INTO user_external_proxy_authorizations(user_id,external_proxy_id) VALUES(?1,?2)",
                params![user, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

pub fn internal_entry_key(id: i64) -> String {
    if id > 0 {
        format!("proxy:{id}")
    } else {
        format!("realm:{}", -id)
    }
}
pub fn internal_entry_id(key: &str) -> Option<i64> {
    let (kind, id) = key.split_once(':')?;
    let id = id.parse::<i64>().ok().filter(|id| *id > 0)?;
    match kind {
        "proxy" => Some(id),
        "realm" => Some(-id),
        _ => None,
    }
}
pub type AllEntryPreferences = std::collections::HashMap<String, (Option<i64>, bool, String)>;
impl Db {
    pub fn all_entry_preferences(&self, user: i64) -> Result<AllEntryPreferences> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT entry_key,position,hidden,custom_name FROM user_entry_preferences WHERE user_id=?1",
        )?;
        let rows = stmt
            .query_map([user], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub fn write_entry_preference(
        &self,
        user: i64,
        key: &str,
        hidden: Option<bool>,
        name: Option<String>,
    ) -> Result<()> {
        let name = name.map(|n| n.trim().to_owned());
        if name.as_ref().is_some_and(|n| n.chars().count() > 100 || n.chars().any(char::is_control)) {
            refuse!("名称最多100字且不能包含控制字符");
        }
        self.conn().execute("INSERT INTO user_entry_preferences(user_id,entry_key,hidden,custom_name) VALUES(?1,?2,COALESCE(?3,0),COALESCE(?4,'')) ON CONFLICT(user_id,entry_key) DO UPDATE SET hidden=COALESCE(?3,hidden),custom_name=COALESCE(?4,custom_name)",params![user,key,hidden,name])?;
        Ok(())
    }
    pub fn active_external_configs(&self, user: i64, now: i64) -> Result<Vec<(i64, serde_json::Value)>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT p.id,p.config FROM external_proxies p JOIN user_external_proxy_authorizations a ON a.external_proxy_id=p.id JOIN users u ON u.id=a.user_id WHERE a.user_id=?1 AND a.enabled=1 AND p.enabled=1 AND u.enabled=1 AND (u.expires_at IS NULL OR u.expires_at>?2) ORDER BY p.id")?;
        let rows = stmt
            .query_map(params![user, now], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|(id, text)| Ok((id, serde_json::from_str(&text)?))).collect()
    }
    pub fn active_entry_keys(&self, user: i64, now: i64) -> Result<HashSet<String>> {
        let mut keys: HashSet<_> =
            self.active_user_portal_proxies(user, now)?.iter().map(|p| internal_entry_key(p.id)).collect();
        keys.extend(self.active_external_configs(user, now)?.iter().map(|(id, _)| format!("external:{id}")));
        Ok(keys)
    }
    pub fn reorder_user_entries(&self, user: i64, keys: &[String], now: i64) -> Result<()> {
        let visible = self.active_entry_keys(user, now)?;
        let requested: HashSet<_> = keys.iter().cloned().collect();
        if keys.len() != requested.len() || requested != visible {
            refuse!("入口列表已变更或排序无效，请刷新后重试");
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (position, key) in keys.iter().enumerate() {
            tx.execute("INSERT INTO user_entry_preferences(user_id,entry_key,position) VALUES(?1,?2,?3) ON CONFLICT(user_id,entry_key) DO UPDATE SET position=excluded.position",params![user,key,position as i64])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn save_user_entry_preference(
        &self,
        user: i64,
        key: &str,
        hidden: Option<bool>,
        name: Option<String>,
    ) -> Result<()> {
        if !self.active_entry_keys(user, Utc::now().timestamp())?.contains(key) {
            refuse!("入口不存在或未授权");
        }
        self.write_entry_preference(user, key, hidden, name)
    }
    pub fn external_authorizations(&self, user: i64) -> Result<Vec<i64>> {
        let conn = self.conn();
        let exists: bool =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)", [user], |r| r.get(0))?;
        if !exists {
            refuse!("用户不存在");
        }
        let mut stmt=conn.prepare("SELECT external_proxy_id FROM user_external_proxy_authorizations WHERE user_id=?1 AND enabled=1 ORDER BY external_proxy_id")?;
        let rows = stmt.query_map([user], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }
    pub fn update_external_proxy(
        &self,
        id: i64,
        config: Option<&serde_json::Value>,
        enabled: Option<bool>,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM external_proxies WHERE id=?1)", [id], |r| r.get(0))?;
        if !exists {
            return Ok(false);
        }
        if let Some(config) = config {
            let config = crate::external_proxy::validate(config)?;
            tx.execute("UPDATE external_proxies SET name=?2,protocol=?3,address=?4,port=?5,config=?6,updated_at=?7 WHERE id=?1",params![id,config["name"].as_str(),config["type"].as_str(),config["server"].as_str(),config["port"].as_i64(),config.to_string(),Utc::now().timestamp()])?;
        }
        if let Some(enabled) = enabled {
            tx.execute(
                "UPDATE external_proxies SET enabled=?2,updated_at=?3 WHERE id=?1",
                params![id, enabled, Utc::now().timestamp()],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }
    pub fn delete_external_proxy(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM user_entry_preferences WHERE entry_key=?1", [format!("external:{id}")])?;
        let deleted = tx.execute("DELETE FROM external_proxies WHERE id=?1", [id])? > 0;
        tx.commit()?;
        Ok(deleted)
    }
}
impl Db {
    pub fn internal_default_names(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut names = {
            let conn = self.conn();
            let mut stmt=conn.prepare("SELECT p.id,p.name,p.include_node_name,COALESCE(n.country,'') FROM proxies p JOIN node n ON n.id=p.node_id")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        displayed_proxy_name(&r.get::<_, String>(1)?, r.get(2)?, &r.get::<_, String>(3)?),
                    ))
                })?
                .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?;
            rows
        };
        for entry in self.realm_subscription_entries()? {
            names.insert(-entry.id, entry.name);
        }
        Ok(names)
    }
}
