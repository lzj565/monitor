use super::*;

pub(super) const PROXY_LIST_ORDER_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS proxy_list_order (key TEXT PRIMARY KEY, position INTEGER NOT NULL);";

fn live_keys(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT 'proxy:' || id FROM proxies ORDER BY sort,id")?;
    let mut keys = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare("SELECT 'realm:' || id FROM realm_rules ORDER BY node_id,id")?;
    keys.extend(stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(db: &Db) -> (i64, i64) {
        let target = db.create_node(&Node { name: "target".into(), ..Default::default() }, "target").unwrap();
        let relay = db.create_node(&Node { name: "relay".into(), ..Default::default() }, "relay").unwrap();
        let proxy = db
            .create_proxy(
                target,
                &ProxyDraft {
                    name: "proxy".into(),
                    include_node_name: false,
                    protocol: "mieru".into(),
                    address_type: "domain".into(),
                    address: "target.example.com".into(),
                    port: 24061,
                    enabled: true,
                    flow: None,
                    config: json!({"transport":"tcp"}),
                },
            )
            .unwrap()
            .unwrap();
        db.realm_save_host(&RealmHostInput { node_id: relay, enabled: true }).unwrap();
        let rule = db
            .realm_save_rule(
                None,
                &RealmRuleInput {
                    node_id: relay,
                    target_proxy_id: proxy,
                    name: "relay".into(),
                    address_type: "domain".into(),
                    address: "relay.example.com".into(),
                    listen_port: 24062,
                    enabled: true,
                },
            )
            .unwrap();
        (proxy, rule)
    }

    #[test]
    fn mixed_order_validates_snapshot_and_restores_backup() {
        let live_file =
            std::env::temp_dir().join(format!("monitor-proxy-order-live-{}.db", rand::random::<u64>()));
        let db = Db::open(live_file.to_str().unwrap()).unwrap();
        let (proxy, rule) = fixture(&db);
        let original = vec![format!("proxy:{proxy}"), format!("realm:{rule}")];
        assert_eq!(db.proxy_list_order().unwrap(), original);
        let reversed = vec![original[1].clone(), original[0].clone()];
        db.save_proxy_list_order(&reversed).unwrap();
        assert_eq!(db.proxy_list_order().unwrap(), reversed);
        for bad in [
            vec![original[0].clone()],
            vec![original[0].clone(), original[0].clone()],
            vec![original[0].clone(), "realm:999999".into()],
        ] {
            assert!(db.save_proxy_list_order(&bad).is_err());
            assert_eq!(db.proxy_list_order().unwrap(), reversed);
        }
        let file = std::env::temp_dir().join(format!("monitor-proxy-order-{}.db", rand::random::<u64>()));
        db.backup_into(file.to_str().unwrap()).unwrap();
        let reopened = Db::open(file.to_str().unwrap()).unwrap();
        assert_eq!(reopened.proxy_list_order().unwrap(), reversed);
        drop(reopened);
        db.save_proxy_list_order(&original).unwrap();
        db.restore_from(file.to_str().unwrap()).unwrap();
        assert_eq!(db.proxy_list_order().unwrap(), reversed);
        std::fs::remove_file(file).unwrap();
        db.conn().execute("DELETE FROM realm_rules WHERE id=?1", [rule]).unwrap();
        assert_eq!(db.proxy_list_order().unwrap(), vec![original[0].clone()]);
        assert!(db.save_proxy_list_order(&reversed).is_err());
        // Newly created rules append; stale orphan ordering never resurrects deleted ids.
        let rule = db
            .realm_save_rule(
                None,
                &RealmRuleInput {
                    node_id: 2,
                    target_proxy_id: proxy,
                    name: "new".into(),
                    address_type: "domain".into(),
                    address: "relay.example.com".into(),
                    listen_port: 24063,
                    enabled: true,
                },
            )
            .unwrap();
        assert_eq!(db.proxy_list_order().unwrap(), vec![original[0].clone(), format!("realm:{rule}")]);
        drop(db);
        std::fs::remove_file(live_file).unwrap();
    }

    #[test]
    fn version_26_backup_migrates_without_order_table() {
        let file = std::env::temp_dir().join(format!("monitor-proxy-order-old-{}.db", rand::random::<u64>()));
        let db = Db::open(file.to_str().unwrap()).unwrap();
        let (proxy, rule) = fixture(&db);
        db.conn().execute_batch("DROP TABLE proxy_list_order; PRAGMA user_version=26;").unwrap();
        drop(db);
        let fresh = Db::open(":memory:").unwrap();
        fresh.check_backup(file.to_str().unwrap()).unwrap();
        fresh.restore_from(file.to_str().unwrap()).unwrap();
        assert_eq!(
            fresh.proxy_list_order().unwrap(),
            vec![format!("proxy:{proxy}"), format!("realm:{rule}")]
        );
        let reopened = Db::open(file.to_str().unwrap()).unwrap();
        assert_eq!(reopened.proxy_list_order().unwrap(), fresh.proxy_list_order().unwrap());
        drop(reopened);
        std::fs::remove_file(file).unwrap();
    }
}
