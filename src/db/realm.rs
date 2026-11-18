use super::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn realm_name(name: &str) -> String {
    format!("{}-realm", name.trim().trim_end_matches("-realm"))
}

fn region(code: &str) -> String {
    let code = code.trim();
    if code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
        code.to_ascii_uppercase()
    } else {
        "XX".into()
    }
}

fn route_name(source: &str, target: &str, custom: &str) -> String {
    format!(
        "{}{}",
        country_flag(target),
        realm_name(&format!(
            "{}-{}-{}",
            region(source),
            region(target),
            custom.trim().trim_end_matches("-realm")
        ))
    )
}

fn unique_realm_names(rows: Vec<(i64, String)>) -> HashMap<i64, String> {
    let reserved: HashSet<_> = rows.iter().map(|(_, name)| name.clone()).collect();
    let mut used = HashSet::new();
    let mut names = HashMap::new();
    let mut ordinals = HashMap::new();
    for (id, base) in rows {
        let mut name = base.clone();
        if used.contains(&name) {
            let number = ordinals.entry(base.clone()).or_insert(2);
            loop {
                name = format!("{}-{number}-realm", base.trim_end_matches("-realm"));
                *number += 1;
                if !reserved.contains(&name) && !used.contains(&name) {
                    break;
                }
            }
        }
        used.insert(name.clone());
        names.insert(id, name);
    }
    names
}

pub(super) fn realm_display_names(conn: &Connection) -> Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT r.id,r.name,COALESCE(NULLIF(s.country_pin,''),s.country),COALESCE(NULLIF(t.country_pin,''),t.country) FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id JOIN node s ON s.id=r.node_id JOIN node t ON t.id=p.node_id ORDER BY r.id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                route_name(&r.get::<_, String>(2)?, &r.get::<_, String>(3)?, &r.get::<_, String>(1)?),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(unique_realm_names(rows))
}

pub(super) const REALM_TABLES: [&str; 2] = ["realm_hosts", "realm_rules"];
pub(super) const REALM_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS realm_hosts (
 node_id INTEGER PRIMARY KEY REFERENCES node(id) ON DELETE RESTRICT,
 enabled INTEGER NOT NULL DEFAULT 1,
 desired_revision INTEGER NOT NULL DEFAULT 0, desired_digest TEXT NOT NULL DEFAULT '',
 applied_revision INTEGER NOT NULL DEFAULT 0, applied_digest TEXT NOT NULL DEFAULT '',
 applied_entries TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT '{}',
 traffic_recheck TEXT NOT NULL DEFAULT '', last_error TEXT NOT NULL DEFAULT '', restart_requested INTEGER NOT NULL DEFAULT 0, request_nonce INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS realm_rules (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 node_id INTEGER NOT NULL REFERENCES realm_hosts(node_id) ON DELETE RESTRICT,
 target_proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE RESTRICT,
 name TEXT NOT NULL, address_type TEXT NOT NULL CHECK(address_type IN ('ipv4','ipv6','domain')), address TEXT NOT NULL,
 listen_host TEXT NOT NULL DEFAULT '::', traffic_key TEXT NOT NULL DEFAULT '',
 listen_port INTEGER NOT NULL CHECK(listen_port BETWEEN 1 AND 65535),
 enabled INTEGER NOT NULL DEFAULT 1, UNIQUE(node_id,listen_port)
);
"#;
/// This UI revision intentionally has no data migration. Only empty legacy
/// Realm tables may be rebuilt; unrelated resources remain in the transaction.
pub(super) fn prepare_realm_schema(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let cols = |table: &str| -> Result<Vec<String>> {
        let mut stmt = tx.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = stmt.query_map([], |r| r.get(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(columns)
    };
    let hosts = cols("realm_hosts")?;
    let rules = cols("realm_rules")?;
    let legacy = hosts.iter().any(|c| c == "public_address")
        || (rules.iter().any(|c| c == "listen_host") && !rules.iter().any(|c| c == "address"));
    if legacy {
        let tables = ["realm_traffic_buckets", "realm_traffic_rules", "realm_rules", "realm_hosts"];
        for table in tables {
            if !cols(table)?.is_empty() {
                let count: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
                if count > 0 {
                    refuse!("旧版 Realm 表 {table} 非空；本版本不迁移旧 Realm 数据，已停止升级，原数据保留");
                }
            }
        }
        // Runtime reports exist even on nodes without Realm rules. Their schema
        // is unchanged, so preserve them and all other accounting tables.
        tx.execute_batch("DROP TABLE IF EXISTS realm_rules; DROP TABLE IF EXISTS realm_hosts;")?;
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmHostInput {
    pub node_id: i64,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealmRuleInput {
    pub node_id: i64,
    pub target_proxy_id: i64,
    pub name: String,
    pub address_type: String,
    pub address: String,
    pub listen_port: u16,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RealmEntry {
    #[serde(default)]
    pub traffic_key: String,
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
    pub accounting: Value,
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
pub(super) fn refresh_realm_credentials(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM user_proxy_authorizations WHERE direct_visible=0", [])?;
    conn.execute("INSERT OR IGNORE INTO user_proxy_authorizations(user_id,proxy_id,enabled,auth,created_at,updated_at,direct_visible) SELECT a.user_id,r.target_proxy_id,1,'{}',?1,?1,0 FROM user_realm_authorizations a JOIN realm_rules r ON r.id=a.rule_id WHERE r.enabled=1",[Utc::now().timestamp()])?;
    Ok(())
}
impl Db {
    pub fn realm_entry_candidates(&self) -> Result<Vec<i64>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT node_id FROM node_components WHERE component='realm' AND installed=1 AND checked_at IS NOT NULL ORDER BY node_id")?;
        let rows = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn realm_hosts(&self) -> Result<Vec<Value>> {
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT h.node_id,n.name,h.enabled,h.desired_revision,h.applied_revision,h.desired_digest,h.applied_digest,h.status,h.last_error FROM realm_hosts h JOIN node n ON n.id=h.node_id ORDER BY h.node_id")?;
        let items=stmt.query_map([],|r|Ok(json!({"node_id":r.get::<_,i64>(0)?,"name":r.get::<_,String>(1)?,"enabled":r.get::<_,bool>(2)?,"desired_revision":r.get::<_,i64>(3)?,"applied_revision":r.get::<_,i64>(4)?,"pending":r.get::<_,String>(5)?!=r.get::<_,String>(6)? || r.get::<_,i64>(3)?!=r.get::<_,i64>(4)?,"status":serde_json::from_str::<Value>(&r.get::<_,String>(7)?).unwrap_or(json!({})),"last_error":r.get::<_,String>(8)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }
    pub fn realm_rules(&self) -> Result<Vec<Value>> {
        let conn = self.conn();
        let names = realm_display_names(&conn)?;
        let mut stmt=conn.prepare("SELECT r.id,r.node_id,r.target_proxy_id,r.name,r.listen_host,r.listen_port,r.enabled,p.node_id,p.name,p.address,p.port,p.enabled,r.traffic_key,r.address_type,r.address FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id ORDER BY r.node_id,r.id")?;
        let items=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"node_id":r.get::<_,i64>(1)?,"target_proxy_id":r.get::<_,i64>(2)?,"name":names.get(&r.get::<_,i64>(0)?),"custom_name":r.get::<_,String>(3)?,"listen_host":r.get::<_,String>(4)?,"listen_port":r.get::<_,i64>(5)?,"enabled":r.get::<_,bool>(6)?,"target_node_id":r.get::<_,i64>(7)?,"target_name":r.get::<_,String>(8)?,"target_address":r.get::<_,String>(9)?,"target_port":r.get::<_,i64>(10)?,"target_enabled":r.get::<_,bool>(11)?,"traffic_key":r.get::<_,String>(12)?,"address_type":r.get::<_,String>(13)?,"address":r.get::<_,String>(14)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(items)
    }
    pub fn realm_save_host(&self, body: &RealmHostInput) -> Result<()> {
        if self.node(body.node_id)?.is_none() {
            refuse!("入口节点不存在");
        }
        self.conn().execute("INSERT INTO realm_hosts(node_id,enabled) VALUES(?1,?2) ON CONFLICT(node_id) DO UPDATE SET enabled=excluded.enabled",params![body.node_id,body.enabled])?;
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
            || !match body.address_type.as_str() {
                "ipv4" => body.address.parse::<std::net::Ipv4Addr>().is_ok(),
                "ipv6" => body.address.parse::<std::net::Ipv6Addr>().is_ok(),
                "domain" => realm_address(&body.address),
                _ => false,
            }
        {
            refuse!("规则名称、对外地址或端口不合法");
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if !tx.query_row("SELECT EXISTS(SELECT 1 FROM node WHERE id=?1)", [body.node_id], |r| {
            r.get::<_, bool>(0)
        })? {
            refuse!("入口节点不存在");
        }
        let target: Option<(i64, bool)> = tx
            .query_row(
                "SELECT node_id,enabled FROM proxies WHERE id=?1 AND protocol NOT IN ('hysteria2','snell')",
                [body.target_proxy_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
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
        // First use enables the node service; existing administrative stops are preserved.
        tx.execute("INSERT OR IGNORE INTO realm_hosts(node_id,enabled) VALUES(?1,1)", [body.node_id])?;
        let args = params![
            body.node_id,
            body.target_proxy_id,
            body.name.trim(),
            body.address_type,
            body.address,
            body.listen_port,
            body.enabled,
            id
        ];
        let saved = if let Some(id) = id {
            tx.execute(
                "UPDATE realm_rules SET traffic_key=lower(hex(randomblob(16))) WHERE id=?1 AND node_id<>?2",
                params![id, body.node_id],
            )?;
            let changed=tx.execute("UPDATE realm_rules SET node_id=?1,target_proxy_id=?2,name=?3,address_type=?4,address=?5,listen_host='::',listen_port=?6,enabled=?7 WHERE id=?8",args)?;
            if changed == 0 {
                refuse!("规则不存在");
            }
            id
        } else {
            tx.execute("INSERT INTO realm_rules(node_id,target_proxy_id,name,address_type,address,listen_port,enabled) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![body.node_id,body.target_proxy_id,body.name.trim(),body.address_type,body.address,body.listen_port,body.enabled])?;
            tx.last_insert_rowid()
        };
        tx.execute(
            "UPDATE realm_rules SET traffic_key=lower(hex(randomblob(16))) WHERE id=?1 AND traffic_key=''",
            [saved],
        )?;
        tx.execute("INSERT INTO realm_traffic_rules(key,node_id,rule_id,name) SELECT traffic_key,node_id,id,name FROM realm_rules WHERE id=?1 ON CONFLICT(key) DO UPDATE SET name=excluded.name",[saved])?;
        if let Some(name) = realm_display_names(&tx)?.get(&saved) {
            tx.execute("UPDATE realm_traffic_rules SET name=?2 WHERE rule_id=?1", params![saved, name])?;
        }
        refresh_realm_credentials(&tx)?;
        tx.commit()?;
        Ok(saved)
    }
    pub fn realm_delete_rule(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if let Some(name) = realm_display_names(&tx)?.get(&id) {
            tx.execute("UPDATE realm_traffic_rules SET name=?2 WHERE rule_id=?1", params![id, name])?;
        }
        tx.execute("DELETE FROM user_entry_preferences WHERE entry_key=?1", [format!("realm:{id}")])?;
        let deleted = tx.execute("DELETE FROM realm_rules WHERE id=?1", [id])? > 0;
        refresh_realm_credentials(&tx)?;
        tx.commit()?;
        Ok(deleted)
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
        let (enabled,revision,old_digest,restart,request):(bool,i64,String,bool,i64)=tx.query_row("SELECT enabled,desired_revision,desired_digest,restart_requested,request_nonce FROM realm_hosts WHERE node_id=?1",[node],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        let recheck: String =
            tx.query_row("SELECT traffic_recheck FROM realm_hosts WHERE node_id=?1", [node], |r| r.get(0))?;
        let mut traffic_rules = Vec::new();
        let source_name: String = tx.query_row("SELECT name FROM node WHERE id=?1", [node], |r| r.get(0))?;
        let mut entries = Vec::new();
        let mut endpoints = Vec::new();
        if enabled {
            let mut stmt=tx.prepare("SELECT r.id,r.target_proxy_id,r.name,r.listen_host,r.listen_port,p.address,p.port,r.traffic_key,r.address FROM realm_rules r JOIN proxies p ON p.id=r.target_proxy_id WHERE r.node_id=?1 AND r.enabled=1 AND p.enabled=1 AND p.node_id<>r.node_id ORDER BY r.id")?;
            let rows = stmt.query_map([node], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?;
            for row in rows {
                let (id, target, name, listen, port, remote, remote_port, traffic_key, address) = row?;
                traffic_rules.push(json!({"key":traffic_key,"listen":listen,"port":port}));
                endpoints
                    .push(json!({"listen":endpoint(&listen,port),"remote":endpoint(&remote,remote_port)}));
                entries.push(RealmEntry {
                    traffic_key,
                    source_node_id: node,
                    source_name: source_name.clone(),
                    id,
                    target_proxy_id: target,
                    name,
                    address,
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
        let accounting_digest = hex::encode(Sha256::digest(serde_json::to_vec(&traffic_rules)?));
        let accounting = json!({"digest":accounting_digest,"recheck":recheck,"rules":traffic_rules});
        Ok(RealmDesired {
            node_id: node,
            revision,
            digest,
            run,
            config,
            entries,
            restart,
            request,
            accounting,
        })
    }
    pub fn realm_reset_applied(&self) -> Result<()> {
        self.conn().execute("UPDATE realm_hosts SET applied_revision=0,applied_digest='',applied_entries='[]',status='{}',traffic_recheck='',last_error='备份恢复后等待 Agent 重新确认'",[])?;
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
        let names = realm_display_names(&conn)?;
        let mut stmt=conn.prepare("SELECT e.value FROM realm_hosts h JOIN json_each(h.applied_entries) e JOIN realm_rules r ON r.id=json_extract(e.value,'$.id') AND r.node_id=h.node_id JOIN proxies p ON p.id=r.target_proxy_id WHERE h.enabled=1 AND r.enabled=1 AND p.enabled=1 AND p.node_id<>r.node_id AND p.id=json_extract(e.value,'$.target_proxy_id') AND p.address=json_extract(e.value,'$.target_address') AND p.port=json_extract(e.value,'$.target_port') ORDER BY h.node_id,r.id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|s| {
                let mut entry: RealmEntry = serde_json::from_str(&s)?;
                entry.name = names.get(&entry.id).context("missing Realm display name")?.clone();
                Ok(entry)
            })
            .collect()
    }
}
