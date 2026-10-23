use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) const REALM_TABLES: [&str; 2] = ["realm_hosts", "realm_rules"];
pub(super) const REALM_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS realm_hosts (
 node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE RESTRICT,
 public_address TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
 desired_revision INTEGER NOT NULL DEFAULT 0, desired_digest TEXT NOT NULL DEFAULT '',
 applied_revision INTEGER NOT NULL DEFAULT 0, applied_digest TEXT NOT NULL DEFAULT '',
 applied_entries TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT '{}',
 last_error TEXT NOT NULL DEFAULT '', restart_requested INTEGER NOT NULL DEFAULT 0, request_nonce INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS realm_rules (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 node_id INTEGER NOT NULL REFERENCES realm_hosts(node_id) ON DELETE RESTRICT,
 target_proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE RESTRICT,
 name TEXT NOT NULL, listen_host TEXT NOT NULL DEFAULT '0.0.0.0',
 listen_port INTEGER NOT NULL CHECK(listen_port BETWEEN 1 AND 65535),
 enabled INTEGER NOT NULL DEFAULT 1, UNIQUE(node_id,listen_port)
);
"#;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmHostInput {
    pub node_id: i64,
    pub public_address: String,
    pub enabled: bool,
    pub takeover: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmRuleInput {
    pub node_id: i64,
    pub target_proxy_id: i64,
    pub name: String,
    pub listen_host: String,
    pub listen_port: u16,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RealmEntry {
    pub source_node_id: i64,
    pub source_name: String,
    pub id: i64,
    pub target_proxy_id: i64,
    pub name: String,
    pub address: String,
    pub port: i64,
    pub target_address: String,
    pub target_port: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct RealmDesired {
    pub node_id: i64,
    pub revision: i64,
    pub digest: String,
    pub run: bool,
    pub config: Value,
    pub entries: Vec<RealmEntry>,
    pub restart: bool,
    pub request: i64,
}

pub fn realm_address(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || (!host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|s| {
                !s.is_empty()
                    && s.len() <= 63
                    && !s.starts_with('-')
                    && !s.ends_with('-')
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }))
}
fn endpoint(host: &str, port: i64) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
impl Db {
    pub fn realm_hosts(&self) -> Result<Vec<Value>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT h.node_id,n.name,h.public_address,h.enabled,h.desired_revision,h.applied_revision,h.desired_digest,h.applied_digest,h.status,h.last_error FROM realm_hosts h JOIN node n ON n.id=h.node_id ORDER BY h.node_id")?;
        let items=stmt.query_map([],|r|Ok(json!({"node_id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"public_address":r.get::<_,String>(2)?,"enabled":r.get::<_,bool>(3)?,"desired_revision":r.get::<_,i64>(4)?,"applied_revision":r.get::<_,i64>(5)?,"pending":r.get::<_,String>(6)?!=r.get::<_,String>(7)? || r.get::<_,i64>(4)?!=r.get::<_,i64>(5)?,"status":serde_json::from_str::<Value>(&r.get::<_,String>(8)?).unwrap_or(json!({})),"last_error":r.get::<_,String>(9)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }
    pub fn realm_rules(&self) -> Result<Vec<Value>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT r.id,r.node_id,r.target_proxy_id,r.name,r.listen_host,r.listen_port,r.enabled,p.node_id,p.name,p.address,p.port,p.enabled FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id ORDER BY r.node_id,r.id")?;
        let items=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"node_id":r.get::<_,i64>(1)?,"target_proxy_id":r.get::<_,i64>(2)?,"name":r.get::<_,String>(3)?,"listen_host":r.get::<_,String>(4)?,"listen_port":r.get::<_,i64>(5)?,"enabled":r.get::<_,bool>(6)?,"target_node_id":r.get::<_,i64>(7)?,"target_name":r.get::<_,String>(8)?,"target_address":r.get::<_,String>(9)?,"target_port":r.get::<_,i64>(10)?,"target_enabled":r.get::<_,bool>(11)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }
    pub fn realm_save_host(&self, body: &RealmHostInput) -> Result<()> {
        if !body.takeover {
            refuse!("请确认由面板管理 Realm 配置");
        }
        if !realm_address(&body.public_address) {
            refuse!("请填写有效的对外域名或 IP，不包含端口");
        }
        if self.node(body.node_id)?.is_none() {
            refuse!("入口节点不存在");
        }
        self.conn().execute("INSERT INTO realm_hosts(node_id,public_address,enabled) VALUES(?1,?2,?3) ON CONFLICT(node_id) DO UPDATE SET public_address=excluded.public_address,enabled=excluded.enabled",params![body.node_id,body.public_address,body.enabled])?;
        Ok(())
    }
    pub fn realm_delete_host(&self, id: i64) -> Result<bool> {
        let conn = self.conn();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM realm_rules WHERE node_id=?1", [id], |r| r.get(0))?;
        if count > 0 {
            refuse!("先删除这台机器关联的转发规则");
        }
        let status: Option<(String, String, String)> = conn
            .query_row(
                "SELECT status,desired_digest,applied_digest FROM realm_hosts WHERE node_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((status, want, got)) = status else { return Ok(false) };
        let value: Value = serde_json::from_str(&status)?;
        if value["running"].as_bool() != Some(false) || want != got {
            refuse!("先删除规则、停用机器并等待停止确认，再移除 Realm 机器");
        }
        conn.execute("DELETE FROM realm_hosts WHERE node_id=?1", [id])?;
        Ok(true)
    }
    pub fn realm_save_rule(&self, id: Option<i64>, body: &RealmRuleInput) -> Result<i64> {
        if body.name.trim().is_empty()
            || body.name.chars().count() > 128
            || body.listen_port == 0
            || !["0.0.0.0", "::"].contains(&body.listen_host.as_str())
        {
            refuse!("规则名称、监听地址或端口不合法");
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM realm_hosts WHERE node_id=?1)",
            [body.node_id],
            |r| r.get::<_, bool>(0),
        )? {
            refuse!("入口 Realm 机器不存在");
        }
        let target: Option<(i64, bool)> = tx
            .query_row("SELECT node_id,enabled FROM proxies WHERE id=?1", [body.target_proxy_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((target_node, enabled)) = target else { refuse!("目标代理不存在") };
        if target_node == body.node_id || (body.enabled && !enabled) {
            refuse!("不能转发到本机代理或已停用代理");
        }
        let conflict: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxies WHERE node_id=?1 AND port=?2 AND enabled=1)",
            params![body.node_id, body.listen_port],
            |r| r.get(0),
        )?;
        if conflict {
            refuse!("监听端口与本机代理冲突");
        }
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM realm_rules WHERE node_id=?1 AND listen_port=?2 AND (?3 IS NULL OR id<>?3))",params![body.node_id,body.listen_port,id],|r|r.get(0))?;
        if duplicate {
            refuse!("Realm 监听端口已被使用");
        }
        let count: i64 =
            tx.query_row("SELECT COUNT(*) FROM realm_rules WHERE node_id=?1", [body.node_id], |r| r.get(0))?;
        if id.is_none() && count >= 1024 {
            refuse!("每台机器最多 1024 条规则");
        }
        let args = params![
            body.node_id,
            body.target_proxy_id,
            body.name.trim(),
            body.listen_host,
            body.listen_port,
            body.enabled,
            id
        ];
        let saved = if let Some(id) = id {
            let changed=tx.execute("UPDATE realm_rules SET node_id=?1,target_proxy_id=?2,name=?3,listen_host=?4,listen_port=?5,enabled=?6 WHERE id=?7",args)?;
            if changed == 0 {
                refuse!("规则不存在");
            }
            id
        } else {
            tx.execute("INSERT INTO realm_rules(node_id,target_proxy_id,name,listen_host,listen_port,enabled) VALUES(?1,?2,?3,?4,?5,?6)",params![body.node_id,body.target_proxy_id,body.name.trim(),body.listen_host,body.listen_port,body.enabled])?;
            tx.last_insert_rowid()
        };
        tx.commit()?;
        Ok(saved)
    }
    pub fn realm_delete_rule(&self, id: i64) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM realm_rules WHERE id=?1", [id])? > 0)
    }
    pub fn realm_refs(&self, node: Option<i64>, proxy: Option<i64>) -> Result<Vec<Value>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT r.id,r.name,r.node_id FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id WHERE r.target_proxy_id=?2 OR r.node_id=?1 OR p.node_id=?1")?;
        let mut rows=stmt.query_map(params![node,proxy],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"realm_node_id":r.get::<_,i64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(id) = node {
            if conn.query_row("SELECT EXISTS(SELECT 1 FROM realm_hosts WHERE node_id=?1)", [id], |r| {
                r.get::<_, bool>(0)
            })? {
                rows.push(json!({"realm_node_id":id,"name":"Realm 机器"}));
            }
        }
        Ok(rows)
    }
    pub fn realm_desired(&self, node: i64) -> Result<RealmDesired> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (address,enabled,revision,old_digest,restart,request):(String,bool,i64,String,bool,i64)=tx.query_row("SELECT public_address,enabled,desired_revision,desired_digest,restart_requested,request_nonce FROM realm_hosts WHERE node_id=?1",[node],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        let source_name: String = tx.query_row("SELECT name FROM node WHERE id=?1", [node], |r| r.get(0))?;
        let mut entries = Vec::new();
        let mut endpoints = Vec::new();
        if enabled {
            let mut stmt=tx.prepare("SELECT r.id,r.target_proxy_id,r.name,r.listen_host,r.listen_port,p.address,p.port FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id WHERE r.node_id=?1 AND r.enabled=1 AND p.enabled=1 AND p.node_id<>r.node_id ORDER BY r.id")?;
            let rows = stmt.query_map([node], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })?;
            for row in rows {
                let (id, target, name, listen, port, remote, remote_port) = row?;
                endpoints
                    .push(json!({"listen":endpoint(&listen,port),"remote":endpoint(&remote,remote_port)}));
                entries.push(RealmEntry {
                    source_node_id: node,
                    source_name: source_name.clone(),
                    id,
                    target_proxy_id: target,
                    name,
                    address: address.clone(),
                    port,
                    target_address: remote,
                    target_port: remote_port,
                });
            }
        }
        let config =
            json!({"log":{"level":"warn"},"network":{"no_tcp":false,"use_udp":false},"endpoints":endpoints});
        let run = !entries.is_empty();
        let digest = hex::encode(Sha256::digest(serde_json::to_vec(
            &json!({"config":config,"entries":entries,"run":run}),
        )?));
        let revision = if digest != old_digest { revision + 1 } else { revision };
        if digest != old_digest {
            tx.execute(
                "UPDATE realm_hosts SET desired_revision=?2,desired_digest=?3 WHERE node_id=?1",
                params![node, revision, digest],
            )?;
        }
        tx.commit()?;
        Ok(RealmDesired { node_id: node, revision, digest, run, config, entries, restart, request })
    }
    pub fn realm_reset_applied(&self) -> Result<()> {
        self.conn().execute("UPDATE realm_hosts SET applied_revision=0,applied_digest='',applied_entries='[]',status='{}',last_error='备份恢复后等待 Agent 重新确认'",[])?;
        Ok(())
    }
    pub fn realm_error(&self, node: i64, error: &str) -> Result<()> {
        self.conn().execute("UPDATE realm_hosts SET last_error=?2 WHERE node_id=?1", params![node, error])?;
        Ok(())
    }
    pub fn realm_observed(&self, node: i64, status: &Value, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE realm_hosts SET status=?2,last_error=?3 WHERE node_id=?1",
            params![node, status.to_string(), error],
        )?;
        Ok(())
    }
    pub fn realm_applied(&self, want: &RealmDesired) -> Result<bool> {
        Ok(self.conn().execute("UPDATE realm_hosts SET applied_revision=?2,applied_digest=?3,applied_entries=?4,last_error='' WHERE node_id=?1 AND desired_revision=?2 AND desired_digest=?3",params![want.node_id,want.revision,want.digest,serde_json::to_string(&want.entries)?])?>0)
    }
    pub fn realm_rebase(&self, node: i64, observed: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE realm_hosts SET desired_revision=MAX(desired_revision,?2)+1 WHERE node_id=?1",
            params![node, observed],
        )?;
        Ok(())
    }
    pub fn realm_request(&self, node: i64, restart: bool) -> Result<()> {
        let changed=self.conn().execute("UPDATE realm_hosts SET request_nonce=request_nonce+1,restart_requested=CASE WHEN ?2 THEN 1 ELSE restart_requested END WHERE node_id=?1",params![node,restart])?;
        if changed == 0 {
            refuse!("Realm 机器不存在");
        }
        Ok(())
    }
    pub fn realm_restart_confirm(&self, node: i64, request: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE realm_hosts SET restart_requested=0 WHERE node_id=?1 AND request_nonce=?2",
            params![node, request],
        )?;
        Ok(())
    }
    pub fn realm_subscription_entries(&self) -> Result<Vec<RealmEntry>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT e.value FROM realm_hosts h JOIN json_each(h.applied_entries) e JOIN realm_rules r ON r.id=json_extract(e.value,'$.id') AND r.node_id=h.node_id JOIN proxies p ON p.id=r.target_proxy_id WHERE h.enabled=1 AND r.enabled=1 AND p.enabled=1 AND p.node_id<>r.node_id AND p.id=json_extract(e.value,'$.target_proxy_id') AND p.address=json_extract(e.value,'$.target_address') AND p.port=json_extract(e.value,'$.target_port') ORDER BY h.node_id,r.id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|s| serde_json::from_str(&s).map_err(Into::into)).collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Db, i64, i64, i64, i64) {
        let db = Db::open(":memory:").unwrap();
        let source =
            db.create_node(&Node { name: "relay".into(), ..Default::default() }, "relay-token").unwrap();
        let target = db
            .create_node(&Node { name: "destination".into(), ..Default::default() }, "target-token")
            .unwrap();
        let proxy=db.create_proxy(target,&ProxyDraft{name:"Reality".into(),include_node_name:false,protocol:"vless".into(),address_type:"domain".into(),address:"target.example.com".into(),port:443,enabled:true,flow:Some("xtls-rprx-vision".into()),config:json!({"reality":{"server_name":"www.example.com","server_port":443,"public_key":"pub","short_id":"abcd"}})}).unwrap().unwrap();
        let user = db
            .create_user(&UserDraft {
                username: "realm-user".into(),
                password_hash: Some("hash".into()),
                enabled: true,
                expires_at: None,
                traffic_limit: None,
                device_limit: None,
                traffic_reset_day: None,
            })
            .unwrap()
            .unwrap();
        db.put_authorization(user, proxy, &AuthorizationDraft { enabled: true, auth: None })
            .unwrap()
            .unwrap();
        db.realm_save_host(&RealmHostInput {
            node_id: source,
            public_address: "relay.example.com".into(),
            enabled: true,
            takeover: true,
        })
        .unwrap();
        (db, source, target, proxy, user)
    }
    fn rule(source: i64, proxy: i64) -> RealmRuleInput {
        RealmRuleInput {
            node_id: source,
            target_proxy_id: proxy,
            name: "via relay".into(),
            listen_host: "::".into(),
            listen_port: 20443,
            enabled: true,
        }
    }
    #[test]
    fn publication_requires_matching_ack_and_live_authorization() {
        let (db, source, _, proxy, user) = fixture();
        let id = db.realm_save_rule(None, &rule(source, proxy)).unwrap();
        let first = db.realm_desired(source).unwrap();
        assert_eq!(first.config["endpoints"][0]["listen"], "[::]:20443");
        assert!(first.run);
        assert_eq!(db.active_user_subscription(user, 0).unwrap().unwrap().1.len(), 1);
        assert!(db.realm_applied(&first).unwrap());
        let (uuid, entries) = db.active_user_subscription(user, 0).unwrap().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].id, -id);
        assert_eq!(entries[1].address, "relay.example.com");
        assert_eq!(entries[0].public_key, entries[1].public_key);
        let portal = db.active_user_portal_proxies(user, 0).unwrap();
        assert_eq!(portal.len(), 2);
        assert_eq!(portal[1].uuid, uuid);
        let mut next = rule(source, proxy);
        next.listen_port = 20444;
        db.realm_save_rule(Some(id), &next).unwrap();
        let second = db.realm_desired(source).unwrap();
        assert!(second.revision > first.revision);
        assert!(!db.realm_applied(&first).unwrap());
        assert_eq!(
            db.active_user_subscription(user, 0).unwrap().unwrap().1[1].port,
            20443,
            "last successful endpoint remains during a failed update"
        );
        assert!(db.realm_applied(&second).unwrap());
        assert_eq!(db.active_user_subscription(user, 0).unwrap().unwrap().1[1].port, 20444);
        db.realm_reset_applied().unwrap();
        assert_eq!(
            db.active_user_subscription(user, 0).unwrap().unwrap().1.len(),
            1,
            "restored snapshots cannot publish until confirmed again"
        );
        db.realm_applied(&second).unwrap();
        db.put_authorization(user, proxy, &AuthorizationDraft { enabled: false, auth: None })
            .unwrap()
            .unwrap();
        assert!(db.active_user_subscription(user, 0).unwrap().unwrap().1.is_empty());
    }
    #[test]
    fn conflicts_references_disabling_and_target_drift() {
        let (db, source, target, proxy, user) = fixture();
        let id = db.realm_save_rule(None, &rule(source, proxy)).unwrap();
        let desired = db.realm_desired(source).unwrap();
        db.realm_applied(&desired).unwrap();
        assert!(db.realm_save_rule(None, &rule(source, proxy)).is_err());
        assert!(!db.realm_refs(Some(target), None).unwrap().is_empty());
        assert!(db.delete_proxy(proxy).is_err());
        assert!(db.delete_node(target).is_err());
        let mut local = rule(source, proxy);
        local.node_id = target;
        assert!(db.realm_save_rule(Some(id), &local).is_err());
        db.conn().execute("UPDATE proxies SET address='new.example.com' WHERE id=?1", [proxy]).unwrap();
        assert_eq!(db.active_user_subscription(user, 0).unwrap().unwrap().1.len(), 1);
        let next = db.realm_desired(source).unwrap();
        assert!(next.revision > desired.revision);
        assert_eq!(next.config["endpoints"][0]["remote"], "new.example.com:443");
        db.realm_applied(&next).unwrap();
        assert_eq!(db.active_user_subscription(user, 0).unwrap().unwrap().1.len(), 2);
        let mut disabled = rule(source, proxy);
        disabled.enabled = false;
        db.realm_save_rule(Some(id), &disabled).unwrap();
        assert!(!db.realm_desired(source).unwrap().run);
        assert_eq!(db.active_user_subscription(user, 0).unwrap().unwrap().1.len(), 1);
        db.realm_delete_rule(id).unwrap();
        assert!(db.realm_delete_host(source).is_err());
    }
    #[test]
    fn schema_21_backup_gains_realm_tables_and_roundtrips_rules() {
        let (db, source, _, proxy, _) = fixture();
        db.realm_save_rule(None, &rule(source, proxy)).unwrap();
        let path = std::env::temp_dir().join(format!("monitor-realm-backup-{}.db", rand::random::<u64>()));
        {
            let mut destination = Connection::open(&path).unwrap();
            rusqlite::backup::Backup::new(&db.conn(), &mut destination)
                .unwrap()
                .run_to_completion(64, std::time::Duration::from_millis(1), None)
                .unwrap();
        }
        db.check_backup(path.to_str().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("DROP TABLE realm_rules; DROP TABLE realm_hosts; PRAGMA user_version=21;")
                .unwrap();
        }
        db.check_backup(path.to_str().unwrap()).unwrap();
        let upgraded = Connection::open(&path).unwrap();
        assert_eq!(upgraded.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 22);
        assert!(!columns_of(&upgraded, "realm_hosts").unwrap().is_empty());
        drop(upgraded);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
