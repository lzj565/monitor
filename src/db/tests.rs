use super::*;

fn db() -> Db {
    Db::open(":memory:").unwrap()
}

/// PRAGMA settings are per connection, so a value read through any other
/// handle proves nothing about the one the hub writes through.
#[test]
fn the_tuning_pragmas_reach_the_connection_the_hub_uses() {
    let db = db();
    let conn = db.conn();
    let read = |p: &str| conn.query_row(&format!("PRAGMA {p}"), [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(read("cache_size"), -8192, "8 MiB of page cache");
    assert_eq!(read("wal_autocheckpoint"), 256);
    assert_eq!(read("journal_size_limit"), 1_048_576);
    assert_eq!(read("busy_timeout"), 5_000);
}

/// A real file, since these three tests exist to exercise what happens to
/// one. Removed by the test that created it.
struct Scratch(String);

impl Scratch {
    fn new() -> Self {
        Self(
            std::env::temp_dir()
                .join(format!("monitor-test-{}.db", rand::random::<u64>()))
                .to_string_lossy()
                .into_owned(),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", ".copy"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0));
        }
    }
}

/// Backup and restore are the two operations that can lose every row in the
/// database, so this exercises the whole path: take a copy, modify the live
/// database, restore the copy, and confirm the change is gone.
#[test]
fn a_backup_restores_the_database_it_was_taken_from() {
    let scratch = Scratch::new();
    let copy = format!("{}.copy", scratch.0);
    let db = Db::open(&scratch.0).unwrap();
    let kept =
        db.create_node(&Node { name: "backed-up".into(), ..Default::default() }, "token-kept").unwrap();
    db.backup_into(&copy).unwrap();

    // Everything after the copy must disappear on restore.
    db.delete_node(kept).unwrap();
    let after = db.create_node(&Node { name: "after".into(), ..Default::default() }, "token-after").unwrap();

    db.check_backup(&copy).unwrap();
    db.restore_from(&copy).unwrap();
    let back = db.nodes().unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!((back[0].name.as_str(), back[0].token.as_str()), ("backed-up", "token-kept"));
    assert!(db.node_by_token("token-after").unwrap().is_none(), "the row made after the copy is gone");

    // The connection remains the hub's: it can write, it is on the schema this
    // build expects, and it retains the journal mode the hub opened with --
    // the copy `VACUUM INTO` wrote is not in WAL mode. Nor does it issue
    // again an id the live file had issued.
    assert!(node(&db, 1) > after, "an id issued before the restore is not issued again");
    let conn = db.conn();
    assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
    assert_eq!(conn.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
    drop(conn);
    let _ = std::fs::remove_file(&copy);
}

/// The upload behind restore is an externally supplied file. Each case here
/// is a way for it not to be a hub backup, and every one must be caught
/// before a single page is copied over live data.
#[test]
fn restore_refuses_anything_that_is_not_a_backup_of_this_hub() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let bad = format!("{}.copy", scratch.0);
    // Each refusal carries a message the panel shows, never a bare 500.
    let refused = |why: &str| {
        let e = db.check_backup(&bad).unwrap_err();
        assert!(e.downcast_ref::<crate::Shown>().is_some(), "{why}: {e:#}");
        e.to_string()
    };

    std::fs::write(&bad, b"this is not a database at all").unwrap();
    refused("not SQLite");

    let _ = std::fs::remove_file(&bad);
    let empty = Connection::open(&bad).unwrap();
    empty.execute_batch("CREATE TABLE unrelated (a)").unwrap();
    refused("SQLite, but not this schema");

    // A file carrying its own code where a table belongs: the restore copies
    // pages, so that schema would become the one the hub runs every statement
    // against.
    empty.execute_batch(&SCHEMA.replace("PRAGMA journal_mode = WAL;", "")).unwrap();
    empty
        .execute_batch("DROP TABLE session; CREATE VIEW session AS SELECT 1 AS token_hash, 2 AS expires_at")
        .unwrap();
    refused("a view where a table belongs");

    // Required tables with the right names and none of the right columns. Every
    // gate above passes: it is a healthy SQLite file, it carries no view or
    // trigger, all required names are present, it stamps itself with this build's
    // version and uses the same page size. Restoring copies pages, so those
    // columns would become the ones the hub runs every statement against,
    // leaving the panel reporting a failed restore over a database already
    // replaced.
    let _ = std::fs::remove_file(&bad);
    let shaped = Connection::open(&bad).unwrap();
    for table in TABLES {
        shaped.execute_batch(&format!("CREATE TABLE {table} (x TEXT)")).unwrap();
    }
    shaped.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")).unwrap();
    // Which table fails first follows the order of TABLES and is incidental;
    // naming the table and the columns is what matters.
    let missing = refused("tables without their columns");
    assert!(missing.contains("表缺少字段"), "{missing}");

    // From a hub carrying a schema this build has never seen.
    let _ = std::fs::remove_file(&bad);
    let newer = Connection::open(&bad).unwrap();
    newer.execute_batch(SCHEMA).unwrap();
    newer.execute_batch(RESOURCE_SCHEMA).unwrap();
    newer.execute_batch(REALM_SCHEMA).unwrap();
    newer.execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1)).unwrap();
    refused("from a newer hub");

    newer.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}")).unwrap();
    db.check_backup(&bad).unwrap();
}

/// `oldest` is what the data page compares against the retention window, so it
/// must span both pruned tables rather than whichever happens to have rows.
#[test]
fn stats_report_the_earliest_history_row_and_the_window_it_is_kept_for() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let id = node(&db, 1);
    let now = Utc::now().timestamp();

    assert_eq!(db.stats().unwrap()["oldest"], serde_json::Value::Null, "no history, no start");
    assert_eq!(db.stats().unwrap()["oldest_ago"], serde_json::Value::Null, "the panel shows a dash");
    assert_eq!(db.stats().unwrap()["retention"], DEFAULT_RETENTION_DAYS, "an unset window is the default");

    db.insert_metric(id, now - 3 * 86_400, &serde_json::json!({"cpu": 1.0})).unwrap();
    assert_eq!(db.stats().unwrap()["oldest"], now - 3 * 86_400);
    let ago = db.stats().unwrap()["oldest_ago"].as_i64().unwrap();
    assert!((ago - 3 * 86_400).abs() <= 1, "counted on the hub's clock");

    // Older, and in the other table: the earlier of the two prevails. The probe
    // must be assigned, or the result is not this node's to file.
    let task = db
        .save_ping_task(&PingTask {
            id: 0,
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![id],
            ..Default::default()
        })
        .unwrap();
    db.insert_pings(id, &[(task, now - 9 * 86_400, 12)]).unwrap();
    assert_eq!(db.stats().unwrap()["oldest"], now - 9 * 86_400);

    // And the hourly tier, which outlives both.
    db.insert_pings(id, &[(task, now - 40 * 86_400, 12)]).unwrap();
    db.roll_up(now, 90).unwrap();
    db.prune(90).unwrap();
    assert_eq!(db.stats().unwrap()["oldest"], (now - 40 * 86_400).div_euclid(3_600) * 3_600);

    db.set("retention_days", "9999").unwrap();
    assert_eq!(db.stats().unwrap()["retention"], MAX_RETENTION_DAYS, "a stored window is still clamped");
}

/// Deleted rows leave free pages behind; only a rebuild returns them to the
/// filesystem, and in WAL mode only after the checkpoint.
#[test]
fn vacuum_gives_the_deleted_pages_back_to_the_filesystem() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let id = node(&db, 1);
    let now = Utc::now().timestamp();
    let sample = serde_json::json!({"cpu": 1.0, "mem_used": 1, "swap_used": 1, "disk_used": 1,
            "net_rx": 1, "net_tx": 1, "tcp": 1, "udp": 1, "procs": 1});
    // Every row strictly before `now`: `prune(0)` cuts at its own `Utc::now()`,
    // and `ts < cutoff` would spare a row stamped in the same second the prune
    // runs.
    for i in 1..=20_000 {
        db.insert_metric(id, now - i, &sample).unwrap();
    }
    let _ = db.conn().query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    let fat = on_disk(&scratch.0);
    // Folded first, as the hourly pass does: minute rows outlive the window
    // until their hour is folded. Dated past the lateness allowance so the
    // last hour counts as complete.
    db.roll_up(now + 3 * 3_600, 1).unwrap();
    db.prune(0).unwrap();

    let freed = db.vacuum().unwrap();
    assert!(freed > 0, "a vacuum after deleting 20 000 rows has to return space");
    assert!(on_disk(&scratch.0) < fat);
    assert_eq!(db.stats().unwrap()["rows"]["metric"], 0);
    assert_eq!(db.nodes().unwrap().len(), 1, "vacuum keeps the rows that are left");
}

fn node(db: &Db, reset_day: u32) -> i64 {
    let token = format!("token-{}", rand::random::<u32>());
    db.create_node(&Node { name: "n".into(), traffic_reset_day: reset_day, ..Default::default() }, &token)
        .unwrap()
}

fn proxy_draft(name: &str, include_node_name: bool) -> ProxyDraft {
    ProxyDraft {
        name: name.into(),
        include_node_name,
        protocol: "vless".into(),
        address_type: "domain".into(),
        address: "proxy.example.com".into(),
        port: 24060,
        enabled: false,
        flow: Some(String::new()),
        config: serde_json::json!({}),
    }
}

fn user_draft(username: &str) -> UserDraft {
    UserDraft {
        username: username.into(),
        password_hash: Some("test-hash".into()),
        enabled: true,
        expires_at: None,
        traffic_limit: None,
        device_limit: None,
        traffic_reset_day: None,
    }
}

#[test]
fn portal_order_is_private_persistent_and_shared_with_subscriptions() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let uid = db.create_user(&user_draft("portal-order")).unwrap().unwrap();
    let other = db.create_user(&user_draft("portal-other")).unwrap().unwrap();
    let nid = node(&db, 1);
    db.conn().execute("UPDATE node SET country='HK' WHERE id=?1", [nid]).unwrap();
    let mut draft = proxy_draft("one", true);
    draft.enabled = true;
    let a = db.create_proxy(nid, &draft).unwrap().unwrap();
    draft.name = "two".into();
    draft.port += 1;
    let b = db.create_proxy(nid, &draft).unwrap().unwrap();
    for uid in [uid, other] {
        for id in [a, b] {
            db.put_authorization(uid, id, &access_draft("")).unwrap().unwrap();
        }
    }
    let now = Utc::now().timestamp();
    db.reorder_user_proxies(uid, &[b, a], now).unwrap();
    for invalid in [vec![a, a], vec![a], vec![a, 99999]] {
        assert!(db.reorder_user_proxies(uid, &invalid, now).is_err());
    }
    assert_eq!(
        db.active_user_portal_proxies(other, now).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![a, b]
    );
    let first = db.active_user_portal_proxies(uid, now).unwrap();
    assert!(first.iter().all(|proxy| proxy.include_node_name));
    assert_eq!(
        crate::db::displayed_proxy_name(&first[0].name, first[0].include_node_name, &first[0].node_country),
        "🇭🇰HK-two"
    );
    db.replace_authorizations(uid, &[(a, true), (b, true)]).unwrap().unwrap();
    drop(db);
    let db = Db::open(&scratch.0).unwrap();
    assert_eq!(
        db.active_user_portal_proxies(uid, now).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![b, a]
    );
    assert_eq!(
        db.active_user_subscription(uid, now).unwrap().unwrap().1.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![b, a]
    );
    draft.name = "three".into();
    draft.port += 1;
    let c = db.create_proxy(nid, &draft).unwrap().unwrap();
    db.put_authorization(uid, c, &access_draft("")).unwrap().unwrap();
    assert_eq!(
        db.active_user_portal_proxies(uid, now).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![b, a, c]
    );
    assert!(db.reorder_user_proxies(other, &[c, b, a], now).is_err());
    db.delete_authorization(uid, b).unwrap();
    assert!(db.reorder_user_proxies(uid, &[b, a, c], now).is_err());
}

#[test]
fn portal_traffic_dates_follow_local_month_end_and_year_boundaries() {
    use chrono::TimeZone;
    let db = db();
    let uid = db.create_user(&user_draft("portal-traffic")).unwrap().unwrap();
    let stamp = |y, m, d| Local.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().timestamp();
    let summary = db.user_traffic_overview(uid, 31, stamp(2024, 2, 1)).unwrap();
    assert_eq!(summary["next_reset_date"], "2024-02-29");
    assert_eq!(summary["reset_days_remaining"], 28);
    assert_eq!(summary["uplink_bytes"], 0);
    assert_eq!(
        db.user_traffic_overview(uid, 31, stamp(2025, 2, 1)).unwrap()["next_reset_date"],
        "2025-02-28"
    );
    assert_eq!(
        db.user_traffic_overview(uid, 1, stamp(2024, 12, 31)).unwrap()["next_reset_date"],
        "2025-01-01"
    );
    assert!(db.user_traffic_overview(uid, 0, stamp(2024, 12, 31)).unwrap()["next_reset_date"].is_null());
    let nid = node(&db, 1);
    db.conn().execute("INSERT INTO proxy_user_traffic(user_id,node_id,uplink_bytes,downlink_bytes,created_at,updated_at) VALUES(?1,?2,12,34,0,0)",params![uid,nid]).unwrap();
    let other = db.create_user(&user_draft("portal-empty")).unwrap().unwrap();
    assert_eq!(db.user_traffic_overview(uid, 0, stamp(2024, 12, 31)).unwrap()["downlink_bytes"], 34);
    assert_eq!(db.user_traffic_overview(other, 0, stamp(2024, 12, 31)).unwrap()["downlink_bytes"], 0);
}

#[test]
fn version_eighteen_backup_gains_portal_order_table() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    db.conn().execute_batch("DROP TABLE user_proxy_order; PRAGMA user_version=18;").unwrap();
    drop(db);
    let db = Db::open(&scratch.0).unwrap();
    assert_eq!(db.stats().unwrap()["rows"]["user_proxy_order"], 0);
}

#[test]
fn all_proxies_include_disabled_rows_across_nodes_in_global_order() {
    let db = db();
    assert!(db.all_proxies().unwrap().is_empty());
    let first_node = node(&db, 1);
    let second_node = node(&db, 2);
    let a = db.create_proxy(first_node, &proxy_draft("all-a", false)).unwrap().unwrap();
    let b = db.create_proxy(second_node, &proxy_draft("all-b", false)).unwrap().unwrap();
    let c = db.create_proxy(first_node, &proxy_draft("all-c", false)).unwrap().unwrap();
    db.reorder_proxies(&[c, b, a]).unwrap();
    let items = db.all_proxies().unwrap();
    assert_eq!(
        items.iter().map(|p| (p.id, p.node_id)).collect::<Vec<_>>(),
        vec![(c, first_node), (b, second_node), (a, first_node)]
    );
    assert!(items.iter().all(|p| !p.enabled));
    db.conn().execute("UPDATE proxies SET sort=0", []).unwrap();
    assert_eq!(db.all_proxies().unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, b, c]);
    db.delete_proxy(b).unwrap();
    assert_eq!(db.all_proxies().unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, c]);
}

#[test]
fn proxies_keep_global_order_append_new_rows_and_reject_stale_orders() {
    let db = db();
    let node_id = node(&db, 1);
    let a = db.create_proxy(node_id, &proxy_draft("order-a", false)).unwrap().unwrap();
    let b = db.create_proxy(node_id, &proxy_draft("order-b", false)).unwrap().unwrap();
    let c = db.create_proxy(node_id, &proxy_draft("order-c", false)).unwrap().unwrap();
    assert_eq!(db.proxies_for_node(node_id).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, b, c]);
    assert_eq!(
        (
            db.proxy(a).unwrap().unwrap().sort,
            db.proxy(b).unwrap().unwrap().sort,
            db.proxy(c).unwrap().unwrap().sort
        ),
        (0, 1, 2)
    );
    db.reorder_proxies(&[c, a, b]).unwrap();
    assert_eq!(db.proxies_for_node(node_id).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![c, a, b]);
    let appended = db.create_proxy(node_id, &proxy_draft("order-appended", false)).unwrap().unwrap();
    assert_eq!(db.proxy(appended).unwrap().unwrap().sort, 3);
    assert_eq!(
        db.proxies_for_node(node_id).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![c, a, b, appended]
    );
    assert!(db.reorder_proxies(&[c, a, a, appended]).is_err(), "duplicate IDs must be rejected");
    assert!(db.reorder_proxies(&[c, a, b]).is_err(), "a stale list missing a proxy must be rejected");
    assert!(db.reorder_proxies(&[c, a, b, 999_999]).is_err(), "unknown IDs must be rejected");
    assert_eq!(
        db.proxies_for_node(node_id).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![c, a, b, appended]
    );
}

#[test]
fn version_sixteen_proxy_order_migration_preserves_existing_id_order() {
    let db = db();
    let node_id = node(&db, 1);
    let a = db.create_proxy(node_id, &proxy_draft("legacy-a", false)).unwrap().unwrap();
    let b = db.create_proxy(node_id, &proxy_draft("legacy-b", false)).unwrap().unwrap();
    db.conn().execute_batch("ALTER TABLE proxies DROP COLUMN sort").unwrap();
    migrate(&db.conn(), 16).unwrap();
    assert_eq!(db.proxies_for_node(node_id).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!((db.proxy(a).unwrap().unwrap().sort, db.proxy(b).unwrap().unwrap().sort), (0, 0));
}

#[test]
fn traffic_node_rows_include_all_proxy_names_without_multiplying_traffic() {
    let db = db();
    let node_id = node(&db, 1);
    let mut first = proxy_draft("first", true);
    first.enabled = true;
    let first_id = db.create_proxy(node_id, &first).unwrap().unwrap();
    let mut second = proxy_draft("second", false);
    second.port = first.port + 1;
    second.enabled = false;
    let second_id = db.create_proxy(node_id, &second).unwrap().unwrap();
    db.conn().execute("INSERT INTO proxy_node_traffic(node_id,uplink_bytes,downlink_bytes,last_uplink_counter,last_downlink_counter,created_at,updated_at) VALUES(?1,50,70,100,200,0,0)", [node_id]).unwrap();
    let page = db.proxy_traffic_node_rows(None, None, "total", true, 1, 100).unwrap();
    assert_eq!(page.total, 1);
    let row = &page.items[0];
    assert_eq!(row.total_bytes, 120);
    assert_eq!(row.proxies.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["first", "second"]);
    assert!(row.proxies[0].enabled);
    assert!(row.proxies[0].include_node_name);
    assert!(!row.proxies[1].enabled);
    assert_eq!(row.proxies[0].protocol, "vless");
    db.reset_proxy_node_traffic(node_id, 123).unwrap();
    let page = db.proxy_traffic_node_rows(None, None, "total", true, 1, 100).unwrap();
    assert_eq!(page.items[0].total_bytes, 0);
    let baseline: i64 = db
        .conn()
        .query_row("SELECT last_uplink_counter FROM proxy_node_traffic WHERE node_id=?1", [node_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(baseline, 100);
    db.delete_proxy(first_id).unwrap();
    db.delete_proxy(second_id).unwrap();
    let page = db.proxy_traffic_node_rows(None, None, "total", true, 1, 100).unwrap();
    assert_eq!(page.total, 1);
    assert!(page.items[0].proxies.is_empty());
}

#[test]
fn user_limits_survive_legacy_updates_and_schedule_changes_do_not_reset_usage() {
    let db = db();
    let mut draft = user_draft("limits");
    draft.traffic_limit = Some(1_610_612_736);
    draft.device_limit = Some(3);
    draft.traffic_reset_day = Some(31);
    let id = db.create_user(&draft).unwrap().unwrap();
    let user = db.user(id).unwrap().unwrap();
    assert_eq!((user.traffic_limit, user.device_limit, user.traffic_reset_day), (1_610_612_736, 3, 31));
    let summary = db.proxy_user_traffic().unwrap().into_iter().find(|row| row.user_id == id).unwrap();
    assert_eq!(summary.traffic_limit, 1_610_612_736);
    assert_eq!(serde_json::to_value(&summary).unwrap()["traffic_limit"], 1_610_612_736_i64);
    let mut legacy = user_draft("limits");
    legacy.enabled = false;
    db.update_user(id, &legacy).unwrap().unwrap();
    let user = db.user(id).unwrap().unwrap();
    assert!(!user.enabled);
    assert_eq!((user.traffic_limit, user.device_limit, user.traffic_reset_day), (1_610_612_736, 3, 31));
    let node_id = node(&db, 1);
    db.conn().execute("INSERT INTO proxy_user_traffic(user_id,node_id,uplink_bytes,downlink_bytes,created_at,updated_at) VALUES(?1,?2,50,70,0,0)", params![id,node_id]).unwrap();
    legacy.traffic_reset_day = Some(1);
    legacy.traffic_limit = Some(0);
    db.update_user(id, &legacy).unwrap().unwrap();
    db.reset_due_user_traffic(Utc::now().timestamp()).unwrap();
    assert_eq!(db.proxy_user_traffic_by_node(id).unwrap()[0].uplink_bytes, 50);
    assert_eq!(db.user(id).unwrap().unwrap().traffic_limit, 0);
}

#[test]
fn scheduled_user_reset_is_atomic_idempotent_and_keeps_counter_baselines() {
    use chrono::TimeZone;
    let db = db();
    let id = db.create_user(&user_draft("monthly")).unwrap().unwrap();
    let node_id = node(&db, 1);
    let mut proxy = proxy_draft("monthly", false);
    proxy.enabled = true;
    let proxy_id = db.create_proxy(node_id, &proxy).unwrap().unwrap();
    db.put_authorization(id, proxy_id, &access_draft("")).unwrap().unwrap();
    db.conn()
        .execute("UPDATE users SET traffic_reset_day=31,traffic_reset_period='2024-01-31' WHERE id=?1", [id])
        .unwrap();
    db.conn().execute("INSERT INTO proxy_user_traffic(user_id,node_id,uplink_bytes,downlink_bytes,last_uplink_counter,last_downlink_counter,created_at,updated_at) VALUES(?1,?2,50,70,100,200,0,0)", params![id,node_id]).unwrap();
    let stamp = |month, day| Local.with_ymd_and_hms(2024, month, day, 12, 0, 0).single().unwrap().timestamp();
    db.reset_due_user_traffic(stamp(2, 28)).unwrap();
    assert_eq!(db.proxy_user_traffic_by_node(id).unwrap()[0].uplink_bytes, 50);
    let counters = HashMap::from([("monthly".into(), ProxyTrafficCounter { uplink: 110, downlink: 225 })]);
    db.record_proxy_traffic(node_id, &counters, &HashMap::new(), stamp(2, 29)).unwrap();
    let row = &db.proxy_user_traffic_by_node(id).unwrap()[0];
    assert_eq!((row.uplink_bytes, row.downlink_bytes), (10, 25));
    db.reset_due_user_traffic(stamp(2, 29)).unwrap();
    assert_eq!(db.proxy_user_traffic_by_node(id).unwrap()[0].uplink_bytes, 10);
    db.reset_proxy_user_traffic(id, stamp(3, 1)).unwrap();
    let period: String = db
        .conn()
        .query_row("SELECT traffic_reset_period FROM users WHERE id=?1", [id], |r| r.get(0))
        .unwrap();
    assert_eq!(period, "2024-02-29");
    // A restart after multiple missed periods clears once on the next pass.
    db.conn().execute("UPDATE proxy_user_traffic SET uplink_bytes=90 WHERE user_id=?1", [id]).unwrap();
    db.reset_due_user_traffic(stamp(5, 5)).unwrap();
    assert_eq!(db.proxy_user_traffic_by_node(id).unwrap()[0].uplink_bytes, 0);
    assert_eq!(period_start(NaiveDate::from_ymd_opt(2025, 2, 28).unwrap(), 31).to_string(), "2025-02-28");
    // Disabling the schedule preserves traffic across future periods.
    db.conn().execute("UPDATE users SET traffic_reset_day=0 WHERE id=?1", [id]).unwrap();
    db.conn().execute("UPDATE proxy_user_traffic SET uplink_bytes=90 WHERE user_id=?1", [id]).unwrap();
    db.reset_due_user_traffic(stamp(6, 30)).unwrap();
    assert_eq!(db.proxy_user_traffic_by_node(id).unwrap()[0].uplink_bytes, 90);
}

#[test]
fn version_fifteen_users_gain_unlimited_defaults_without_losing_identity() {
    let db = db();
    let id = db.create_user(&user_draft("migrated-limits")).unwrap().unwrap();
    let uuid = db.user(id).unwrap().unwrap().uuid;
    for name in ["traffic_limit", "device_limit", "traffic_reset_day", "traffic_reset_period"] {
        db.conn().execute_batch(&format!("ALTER TABLE users DROP COLUMN {name}")).unwrap();
    }
    migrate(&db.conn(), 15).unwrap();
    let user = db.user(id).unwrap().unwrap();
    assert_eq!(user.uuid, uuid);
    assert_eq!((user.traffic_limit, user.device_limit, user.traffic_reset_day), (0, 0, 0));
}

fn access_draft(flow: &str) -> AuthorizationDraft {
    AuthorizationDraft { enabled: true, auth: Some(serde_json::json!({"flow":flow})) }
}

#[test]
fn auto_authorize_new_proxies_is_opt_in_and_preserves_manual_selection() {
    let db = db();
    let uid = db.create_user(&user_draft("auto-proxies")).unwrap().unwrap();
    let other = db.create_user(&user_draft("manual-proxies")).unwrap().unwrap();
    let nid = node(&db, 1);
    let old = db.create_proxy(nid, &proxy_draft("existing", false)).unwrap().unwrap();
    assert!(!db.user(uid).unwrap().unwrap().auto_authorize_new_proxies);
    db.replace_authorizations_with_policy(uid, &[], Some(true)).unwrap().unwrap();
    assert_eq!(db.user(uid).unwrap().unwrap().proxy_count, 0);
    db.conn().execute("UPDATE users SET enabled=0,expires_at=1 WHERE id=?1", [uid]).unwrap();
    let added = db.create_proxy(nid, &proxy_draft("new-disabled", false)).unwrap().unwrap();
    let accesses = db.authorizations_for_user(uid).unwrap().unwrap();
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].proxy.id, added);
    assert!(accesses[0].access.enabled);
    assert_eq!(db.user(other).unwrap().unwrap().proxy_count, 0);
    db.replace_authorizations(uid, &[(old, true), (added, true)]).unwrap().unwrap();
    assert!(db.user(uid).unwrap().unwrap().auto_authorize_new_proxies, "legacy writes preserve the policy");
    assert!(matches!(
        db.replace_authorizations_with_policy(uid, &[(i64::MAX, true)], Some(false)).unwrap(),
        Err(AuthorizationWriteIssue::ProxyNotFound)
    ));
    assert!(db.user(uid).unwrap().unwrap().auto_authorize_new_proxies);
    assert_eq!(db.user(uid).unwrap().unwrap().proxy_count, 2);
    db.replace_authorizations_with_policy(uid, &[(old, true), (added, true)], Some(false)).unwrap().unwrap();
    db.create_proxy(nid, &proxy_draft("after-off", false)).unwrap().unwrap();
    assert_eq!(db.user(uid).unwrap().unwrap().proxy_count, 2);
}

#[test]
fn auto_authorize_new_proxies_rolls_back_proxy_and_policy_on_failure() {
    let db = db();
    let uid = db.create_user(&user_draft("atomic-auto")).unwrap().unwrap();
    let nid = node(&db, 1);
    let old = db.create_proxy(nid, &proxy_draft("atomic-existing", false)).unwrap().unwrap();
    db.replace_authorizations_with_policy(uid, &[(old, true)], Some(true)).unwrap().unwrap();
    db.conn()
        .execute_batch(
            "CREATE TRIGGER fail_auto BEFORE INSERT ON user_proxy_authorizations
            BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
    assert!(db.create_proxy(nid, &proxy_draft("atomic-new", false)).is_err());
    assert_eq!(db.all_proxies().unwrap().len(), 1);
    assert!(db.replace_authorizations_with_policy(uid, &[(old, true)], Some(false)).is_err());
    assert_eq!(db.user(uid).unwrap().unwrap().proxy_count, 1);
    assert!(db.user(uid).unwrap().unwrap().auto_authorize_new_proxies);
}

#[test]
fn version_twenty_auto_authorize_migration_defaults_off_and_is_repeatable() {
    let db = db();
    let uid = db.create_user(&user_draft("migrated-auto")).unwrap().unwrap();
    db.conn()
        .execute_batch("ALTER TABLE users DROP COLUMN auto_authorize_new_proxies; PRAGMA user_version=19;")
        .unwrap();
    migrate(&db.conn(), 19).unwrap();
    assert!(!db.user(uid).unwrap().unwrap().auto_authorize_new_proxies);
    db.replace_authorizations_with_policy(uid, &[], Some(true)).unwrap().unwrap();
    migrate(&db.conn(), 19).unwrap();
    assert!(db.user(uid).unwrap().unwrap().auto_authorize_new_proxies);
}

#[test]
fn replacing_authorizations_is_atomic_and_returns_old_and_new_nodes_once() {
    let db = db();
    let user_id = db.create_user(&user_draft("replace-me")).unwrap().unwrap();
    let old_node = node(&db, 1);
    let new_node = node(&db, 1);
    let shared_node = node(&db, 1);
    let mut draft = proxy_draft("old", false);
    draft.enabled = true;
    let old_proxy = db.create_proxy(old_node, &draft).unwrap().unwrap();
    draft.name = "new".into();
    draft.port += 1;
    let new_proxy = db.create_proxy(new_node, &draft).unwrap().unwrap();
    draft.name = "shared".into();
    draft.port += 1;
    let shared_proxy = db.create_proxy(shared_node, &draft).unwrap().unwrap();
    draft.name = "shared two".into();
    draft.port += 1;
    let shared_proxy_two = db.create_proxy(shared_node, &draft).unwrap().unwrap();
    db.put_authorization(user_id, old_proxy, &access_draft("")).unwrap().unwrap();

    let affected = db
        .replace_authorizations(
            user_id,
            &[(new_proxy, true), (shared_proxy, true), (shared_proxy_two, false)],
        )
        .unwrap()
        .unwrap();
    assert_eq!(affected, vec![old_node, new_node, shared_node]);
    let current = db.authorizations_for_user(user_id).unwrap().unwrap();
    assert_eq!(current.len(), 3);
    assert!(current.iter().any(|item| item.proxy.id == shared_proxy_two && !item.access.enabled));

    assert_eq!(
        db.replace_authorizations(user_id, &[(i64::MAX, true)]).unwrap(),
        Err(AuthorizationWriteIssue::ProxyNotFound)
    );
    assert_eq!(db.authorizations_for_user(user_id).unwrap().unwrap().len(), 3);
}

#[test]
fn expired_authorized_users_are_returned_only_when_enabled_on_the_node() {
    let db = db();
    let now = Utc::now().timestamp();
    let mut expired = user_draft("expired-on-node");
    expired.expires_at = Some(now - 1);
    let user_id = db.create_user(&expired).unwrap().unwrap();
    let node_id = node(&db, 1);
    let mut draft = proxy_draft("active-proxy", false);
    draft.enabled = true;
    let proxy_id = db.create_proxy(node_id, &draft).unwrap().unwrap();
    db.put_authorization(user_id, proxy_id, &access_draft("")).unwrap().unwrap();

    assert!(db.expired_authorized_usernames_for_node(node_id, now).unwrap().contains("expired-on-node"));
    db.put_authorization(user_id, proxy_id, &AuthorizationDraft { enabled: false, auth: None })
        .unwrap()
        .unwrap();
    assert!(db.expired_authorized_usernames_for_node(node_id, now).unwrap().is_empty());
}

#[test]
fn users_own_one_stable_uuid_across_authorizations_and_config_generation() {
    let db = db();
    let u1 = db.create_user(&user_draft("alice")).unwrap().unwrap();
    let u2 = db.create_user(&user_draft("bob")).unwrap().unwrap();
    let uuid = db.user(u1).unwrap().unwrap().uuid;
    let uuid2 = db.user(u2).unwrap().unwrap().uuid;
    assert_ne!(uuid, uuid2);
    assert_eq!(uuid.len(), 36);
    assert_eq!(uuid.as_bytes()[14], b'4');
    assert!(matches!(uuid.as_bytes()[19], b'8' | b'9' | b'a' | b'b'), "variant nibble uses RFC 4122 variant");
    assert_eq!(db.create_user(&user_draft("alice")).unwrap(), Err(UserWriteIssue::UsernameConflict));

    let n = node(&db, 1);
    db.conn().execute("UPDATE node SET group_name='Asia',country='HK' WHERE id=?1", [n]).unwrap();
    let mut draft = proxy_draft("p1", false);
    draft.enabled = true;
    draft.port = 24061;
    draft.flow = Some("xtls-rprx-vision".into());
    let p1 = db.create_proxy(n, &draft).unwrap().unwrap();
    draft.name = "p2".into();
    draft.port = 24062;
    draft.flow = Some(String::new());
    let p2 = db.create_proxy(n, &draft).unwrap().unwrap();
    draft.name = "unassigned".into();
    draft.port = 24063;
    let unassigned = db.create_proxy(n, &draft).unwrap().unwrap();
    db.put_authorization(u1, p1, &access_draft("xtls-rprx-vision")).unwrap().unwrap();
    db.put_authorization(u1, p2, &access_draft("")).unwrap().unwrap();
    db.put_authorization(u2, unassigned, &access_draft("")).unwrap().unwrap();
    assert_eq!(db.user(u1).unwrap().unwrap().uuid, uuid, "adding authorization keeps UUID");
    for proxy_id in [p1, p2] {
        let users = db.active_users_for_proxy(proxy_id, Utc::now().timestamp()).unwrap();
        assert_eq!(users[0]["uuid"], uuid);
    }
    let own_nodes = db.active_user_portal_proxies(u1, Utc::now().timestamp()).unwrap();
    assert_eq!(own_nodes.iter().map(|proxy| proxy.id).collect::<Vec<_>>(), vec![p1, p2]);
    assert_eq!(own_nodes[0].node_group, "Asia");
    assert_eq!(own_nodes[0].node_country, "HK");
    assert!(own_nodes
        .iter()
        .all(|proxy| !serde_json::to_value(proxy).unwrap().to_string().contains("private_key")));
    let (subscription_uuid, own_subscription) =
        db.active_user_subscription(u1, Utc::now().timestamp()).unwrap().unwrap();
    assert_eq!(subscription_uuid, uuid);
    assert_eq!(own_subscription.iter().map(|proxy| proxy.id).collect::<Vec<_>>(), vec![p1, p2]);
    assert_eq!(
        own_subscription.iter().map(|proxy| proxy.flow.as_str()).collect::<Vec<_>>(),
        vec!["xtls-rprx-vision", ""]
    );
    assert!(!own_subscription.iter().any(|proxy| proxy.id == unassigned));
    assert_eq!(db.active_user_portal_proxies(u2, Utc::now().timestamp()).unwrap()[0].id, unassigned);
    assert!(db.delete_authorization(u1, p1).unwrap());
    assert_eq!(db.user(u1).unwrap().unwrap().uuid, uuid, "deleting authorization keeps UUID");
    db.put_authorization(u1, p1, &access_draft("")).unwrap().unwrap();
    assert_eq!(db.user(u1).unwrap().unwrap().uuid, uuid, "regrant keeps UUID");
    assert!(
        db.conn().execute("UPDATE users SET uuid=?2 WHERE id=?1", params![u2, uuid]).is_err(),
        "UUID has a database unique constraint"
    );
    let cols = columns_of(&db.conn(), "user_proxy_authorizations").unwrap();
    assert!(!cols.contains("uuid"));
    assert!(!cols.contains("user_proxy_access"));
    let duplicate = db.conn().execute(
            "INSERT INTO user_proxy_authorizations(user_id,proxy_id,enabled,auth,created_at,updated_at) VALUES(?1,?2,1,'{}',1,1)",
            params![u1,p1],
        );
    assert!(duplicate.is_err(), "user/proxy authorization is unique");

    let nodes = db.reset_user_uuid(u1).unwrap().unwrap().1;
    let replacement = db.user(u1).unwrap().unwrap().uuid;
    assert_eq!(nodes, vec![n]);
    assert_ne!(replacement, uuid);
    assert_eq!(db.active_users_for_proxy(p1, Utc::now().timestamp()).unwrap()[0]["uuid"], replacement);
    db.delete_user(u1).unwrap();
    assert_eq!(
        db.conn()
            .query_row("SELECT COUNT(*) FROM user_proxy_authorizations WHERE user_id=?1", [u1], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn default_admin_is_seeded_idempotently_and_cannot_be_renamed_or_deleted() {
    let db = db();
    let admin = db.users(None, None).unwrap().into_iter().find(|user| user.username == "admin").unwrap();
    assert!(admin.enabled);
    assert_eq!(admin.expires_at, None);
    assert_eq!(admin.uuid.len(), 36);
    let original_uuid = admin.uuid.clone();

    assert_eq!(db.create_user(&user_draft("admin")).unwrap(), Err(UserWriteIssue::UsernameConflict));
    assert_eq!(
        db.update_user(admin.id, &user_draft("renamed")).unwrap(),
        Err(UserWriteIssue::AdminUsernameLocked)
    );
    assert_eq!(db.delete_user(admin.id).unwrap(), UserDeleteResult::Protected);

    let editable = user_draft("admin");
    assert_eq!(db.update_user(admin.id, &editable).unwrap(), Ok(()), "other admin fields remain editable");
    assert_eq!(db.user(admin.id).unwrap().unwrap().uuid, original_uuid);

    // Re-running startup migration leaves the existing built-in user intact.
    migrate(&db.conn(), SCHEMA_VERSION).unwrap();
    assert_eq!(db.user(admin.id).unwrap().unwrap().uuid, original_uuid);
    assert_eq!(db.users(None, None).unwrap().iter().filter(|user| user.username == "admin").count(), 1);

    // Even if a database is edited outside the API, the next startup repairs it.
    db.conn().execute("DELETE FROM users WHERE id=?1", [admin.id]).unwrap();
    migrate(&db.conn(), SCHEMA_VERSION).unwrap();
    let restored = db.users(None, None).unwrap().into_iter().find(|user| user.username == "admin").unwrap();
    assert_ne!(restored.uuid, original_uuid);
    assert_eq!(restored.expires_at, None);
}

#[test]
fn user_uuids_are_generated_and_only_reset_explicitly() {
    let db = db();
    let alice = db.create_user(&user_draft("alice")).unwrap().unwrap();
    let uuid = db.user(alice).unwrap().unwrap().uuid;
    assert_eq!(db.update_user(alice, &user_draft("alice")).unwrap(), Ok(()));
    assert_eq!(db.user(alice).unwrap().unwrap().uuid, uuid, "omitted UUID is preserved");
    let replacement = db.reset_user_uuid(alice).unwrap().unwrap().0;
    assert_ne!(replacement, uuid);
    assert_eq!(db.user(alice).unwrap().unwrap().uuid, replacement);
}

#[test]
fn user_sessions_are_role_scoped_and_revoked_on_credential_or_status_changes() {
    let db = db();
    let id = db.create_user(&user_draft("session-user")).unwrap().unwrap();
    let uuid = db.user(id).unwrap().unwrap().uuid;
    db.create_user_session("user-hash", Utc::now().timestamp() + 3600, id).unwrap();
    assert_eq!(db.session_principal("user-hash"), Some(SessionPrincipal::User(id)));
    assert!(!db.admin_session_valid("user-hash"));
    db.create_session("admin-hash", Utc::now().timestamp() + 3600).unwrap();
    assert_eq!(db.session_principal("admin-hash"), Some(SessionPrincipal::Admin));
    assert_eq!(db.user_session_user_id("admin-hash"), None);

    let before = db.user(id).unwrap().unwrap();
    let changed_password = UserDraft {
        username: before.username,
        password_hash: Some("replacement-hash".into()),
        enabled: true,
        expires_at: None,
        traffic_limit: None,
        device_limit: None,
        traffic_reset_day: None,
    };
    assert_eq!(db.update_user(id, &changed_password).unwrap(), Ok(()));
    assert_eq!(db.session_principal("user-hash"), None, "password changes revoke user sessions");
    assert_eq!(db.user(id).unwrap().unwrap().uuid, uuid, "password changes preserve VLESS identity");

    db.create_user_session("user-hash-2", Utc::now().timestamp() + 3600, id).unwrap();
    let disabled = UserDraft {
        username: "session-user".into(),
        password_hash: None,
        enabled: false,
        expires_at: None,
        traffic_limit: None,
        device_limit: None,
        traffic_reset_day: None,
    };
    assert_eq!(db.update_user(id, &disabled).unwrap(), Ok(()));
    assert_eq!(db.session_principal("user-hash-2"), None, "disabling revokes user sessions");
    assert_eq!(db.user(id).unwrap().unwrap().uuid, uuid);
}

#[test]
fn migration_discards_old_per_proxy_uuids_and_clears_legacy_authorizations() {
    let scratch = Scratch::new();
    let old = Connection::open(&scratch.0).unwrap();
    old.execute_batch("PRAGMA foreign_keys=ON;
            CREATE TABLE users(id INTEGER PRIMARY KEY,username TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1,expires_at INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL);
            CREATE TABLE proxies(id INTEGER PRIMARY KEY,node_id INTEGER NOT NULL,name TEXT NOT NULL);
            CREATE TABLE user_proxy_access(id INTEGER PRIMARY KEY,user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,proxy_id INTEGER NOT NULL REFERENCES proxies(id) ON DELETE CASCADE,enabled INTEGER NOT NULL,auth TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,UNIQUE(user_id,proxy_id));
            INSERT INTO users VALUES(1,'legacy',1,NULL,1,1);
            INSERT INTO users VALUES(2,'empty',1,NULL,1,1);
            INSERT INTO proxies VALUES(1,1,'a'); INSERT INTO proxies VALUES(2,2,'b');
            INSERT INTO user_proxy_access VALUES(11,1,1,1,'{\"uuid\":\"11111111-1111-4111-8111-111111111111\",\"flow\":\"\"}',10,10);
            INSERT INTO user_proxy_access VALUES(12,1,2,1,'{\"uuid\":\"22222222-2222-4222-8222-222222222222\",\"flow\":\"xtls-rprx-vision\"}',20,20);
            PRAGMA user_version=11;").unwrap();
    drop(old);
    let db = Db::open(&scratch.0).unwrap();
    let user = db.user(1).unwrap().unwrap();
    assert!(user.uuid.contains('-') && user.uuid.len() == 36);
    let empty_user = db.user(2).unwrap().unwrap();
    assert!(empty_user.uuid.len() == 36);
    assert_ne!(empty_user.uuid, user.uuid);
    let conn = db.conn();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM user_proxy_authorizations", [], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM users WHERE id IN (1,2)", [], |r| r.get::<_, i64>(0)).unwrap(),
        2
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM proxies", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert!(!columns_of(&conn, "user_proxy_authorizations").unwrap().contains("uuid"));
    assert!(conn
        .query_row("SELECT COUNT(*) FROM user_proxy_access", [], |r| r.get::<_, i64>(0))
        .unwrap_err()
        .to_string()
        .contains("no such table"));
    assert!(conn
        .query_row("SELECT value FROM setting WHERE key='migration_v15_authorizations_reset'", [], |r| {
            r.get::<_, String>(0)
        })
        .is_ok());
    drop(conn);
    // A downgraded binary stamps version 14 on startup. Re-upgrading must
    // not delete grants made after the first reset.
    db.put_authorization(1, 1, &AuthorizationDraft { enabled: true, auth: None }).unwrap().unwrap();
    db.conn().execute_batch("PRAGMA user_version=14").unwrap();
    migrate(&db.conn(), 14).unwrap();
    assert_eq!(
        db.conn()
            .query_row("SELECT COUNT(*) FROM user_proxy_authorizations", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(db);
    let db = Db::open(&scratch.0).unwrap();
    assert_eq!(db.user(1).unwrap().unwrap().uuid, user.uuid, "rerunning migration does not rotate UUID");
    assert_eq!(
        db.conn()
            .query_row("SELECT COUNT(*) FROM user_proxy_authorizations", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn proxy_names_are_unique_by_the_final_global_case_insensitive_label() {
    let db = db();
    let (hk, jp, duplicate_hk) = (node(&db, 1), node(&db, 1), node(&db, 1));
    {
        let conn = db.conn();
        conn.execute("UPDATE node SET name='HK服务器',country='HK' WHERE id=?1", [hk]).unwrap();
        conn.execute("UPDATE node SET name='JP服务器',country='JP' WHERE id=?1", [jp]).unwrap();
        conn.execute("UPDATE node SET name='HK服务器',country='HK' WHERE id=?1", [duplicate_hk]).unwrap();
    }

    let id = db.create_proxy(hk, &proxy_draft("Reality", true)).unwrap().unwrap();
    assert!(db.proxy(id).unwrap().unwrap().include_node_name);
    assert!(db.create_proxy(jp, &proxy_draft("reality", true)).unwrap().is_ok());
    assert_eq!(
        db.create_proxy(duplicate_hk, &proxy_draft("reality", true)).unwrap(),
        Err(ProxyWriteIssue::NameConflict),
        "the generated HK labels collide across nodes",
    );

    let plain = node(&db, 1);
    assert!(db.create_proxy(plain, &proxy_draft("Common", false)).unwrap().is_ok());
    assert_eq!(
        db.create_proxy(jp, &proxy_draft(" COMMON ", false)).unwrap(),
        Err(ProxyWriteIssue::NameConflict),
        "unprefixed names are compared globally after trimming and case folding",
    );
    assert_eq!(
        db.update_proxy(id, None, &proxy_draft(" common ", false)).unwrap(),
        Err(ProxyWriteIssue::NameConflict),
        "editing to another proxy's final name is refused",
    );
    assert_eq!(db.update_proxy(id, None, &proxy_draft("REALITY", true)).unwrap(), Ok(()));
}

#[test]
fn proxy_flow_round_trips_and_omitted_legacy_updates_preserve_it() {
    let db = db();
    let node_id = node(&db, 1);
    let mut draft = proxy_draft("Flow", false);
    draft.flow = Some("xtls-rprx-vision".into());
    let id = db.create_proxy(node_id, &draft).unwrap().unwrap();
    assert_eq!(db.proxy(id).unwrap().unwrap().flow, "xtls-rprx-vision");

    draft.flow = None;
    db.update_proxy(id, None, &draft).unwrap().unwrap();
    assert_eq!(
        db.proxy(id).unwrap().unwrap().flow,
        "xtls-rprx-vision",
        "an old client omitting Flow does not clear it"
    );

    draft.flow = Some(String::new());
    db.update_proxy(id, None, &draft).unwrap().unwrap();
    assert_eq!(db.proxy(id).unwrap().unwrap().flow, "", "an explicit empty Flow clears it");
}

#[test]
fn updating_proxy_can_move_it_to_another_node_and_checks_target_port() {
    let db = db();
    let (source, target) = (node(&db, 1), node(&db, 1));
    {
        let conn = db.conn();
        conn.execute("UPDATE node SET name='HK服务器',country='HK' WHERE id=?1", [source]).unwrap();
        conn.execute("UPDATE node SET name='JP服务器',country='JP' WHERE id=?1", [target]).unwrap();
    }
    let mut moving = proxy_draft("Moving", true);
    moving.enabled = true;
    let id = db.create_proxy(source, &moving).unwrap().unwrap();

    let mut occupied = proxy_draft("Occupied", false);
    occupied.enabled = true;
    let occupied_id = db.create_proxy(target, &occupied).unwrap().unwrap();
    assert_eq!(db.update_proxy(id, Some(target), &moving).unwrap(), Err(ProxyWriteIssue::PortConflict));
    assert_eq!(
        db.proxy(id).unwrap().unwrap().node_id,
        source,
        "a rejected move leaves the source node intact"
    );

    {
        let conn = db.conn();
        conn.execute("UPDATE proxies SET name='Moving',include_node_name=1 WHERE id=?1", [occupied_id])
            .unwrap();
    }
    {
        let conn = db.conn();
        conn.execute("UPDATE node SET name='HK服务器',country='HK' WHERE id=?1", [target]).unwrap();
    }
    moving.port = 24061;
    assert_eq!(db.update_proxy(id, Some(target), &moving).unwrap(), Err(ProxyWriteIssue::NameConflict));
    assert_eq!(
        db.proxy(id).unwrap().unwrap().node_id,
        source,
        "a name collision leaves the source node intact"
    );

    {
        let conn = db.conn();
        conn.execute("UPDATE node SET name='JP服务器',country='JP' WHERE id=?1", [target]).unwrap();
        conn.execute("UPDATE proxies SET name='Occupied',include_node_name=0 WHERE id=?1", [occupied_id])
            .unwrap();
    }
    assert_eq!(db.update_proxy(id, Some(target), &moving).unwrap(), Ok(()));
    assert_eq!(db.proxy(id).unwrap().unwrap().node_id, target);
    assert_eq!(db.update_proxy(id, Some(i64::MAX), &moving).unwrap(), Err(ProxyWriteIssue::NodeNotFound));
    assert_eq!(db.proxy(id).unwrap().unwrap().node_id, target);
}

#[test]
fn version_ten_proxies_migrate_without_changing_their_existing_names() {
    let scratch = Scratch::new();
    let old = Connection::open(&scratch.0).unwrap();
    old.execute_batch(SCHEMA).unwrap();
    old.execute_batch(
            "CREATE TABLE proxies (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
               name TEXT NOT NULL, protocol TEXT NOT NULL, address_type TEXT NOT NULL,
               address TEXT NOT NULL, port INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
               config TEXT NOT NULL DEFAULT '{}', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             INSERT INTO node(id,name,token,created_at) VALUES(1,'HK服务器','token',1);
             INSERT INTO proxies(id,node_id,name,protocol,address_type,address,port,enabled,config,created_at,updated_at)
               VALUES(1,1,'🇭🇰 [HK服务器] Reality','vless','domain','hk.example.com',24060,1,'{}',1,1);
             PRAGMA user_version=10;",
        )
        .unwrap();
    drop(old);

    let db = Db::open(&scratch.0).unwrap();
    let migrated = db.proxy(1).unwrap().unwrap();
    assert_eq!(migrated.name, "🇭🇰 [HK服务器] Reality");
    assert!(!migrated.include_node_name, "pre-existing names remain unchanged after upgrade");
}

/// The country is derived from the address, so it must be dropped the moment
/// the address no longer matches -- and only then, or every reconnect would
/// spend an outbound request repeating a settled lookup.
#[test]
fn a_country_outlives_a_reconnect_and_dies_with_the_address_it_came_from() {
    let db = db();
    let id = node(&db, 1);
    let facts = serde_json::json!({"hostname": "h"});
    let save = |ip: &str| db.save_facts(id, &facts, ip, ip).unwrap();
    let stored = || db.node(id).unwrap().unwrap().country;

    assert!(save("198.51.100.4"), "a node with no country is owed a lookup");
    db.set_country(id, "US", "198.51.100.4").unwrap();
    assert!(!save("198.51.100.4"), "the same address asks nothing a second time");
    assert_eq!(stored(), "US");
    assert!(save("203.0.113.9"), "a new address is a new question");
    assert_eq!(stored(), "", "and the old answer no longer shows");
    assert!(db.country_owed(id, "203.0.113.9").unwrap(), "owed until an answer lands");
    assert!(!db.country_owed(id, "198.51.100.4").unwrap(), "nothing is owed for an address left behind");

    // A lookup issued for the old address, arriving after the move.
    db.set_country(id, "US", "198.51.100.4").unwrap();
    assert_eq!(stored(), "", "an answer about an address the node has left is dropped");
    db.set_country(id, "JP", "203.0.113.9").unwrap();
    assert_eq!(stored(), "JP", "the answer about the address it is at now lands");
    assert!(!db.country_owed(id, "203.0.113.9").unwrap());

    // The source, not the connection address, is what the country belongs to:
    // a proxy exit changing under a node with a public interface address
    // leaves the badge alone.
    assert!(!db.save_facts(id, &facts, "198.51.100.77", "203.0.113.9").unwrap());
    assert_eq!(stored(), "JP");
    // Nothing public to look up: no country, and none owed.
    assert!(!db.save_facts(id, &facts, "192.168.1.2", "").unwrap());
    assert_eq!(stored(), "");
}

/// A reboot: the first hello carries only the v6, the next one the v4 again.
/// The detour spends the node's hourly lookup, so the address returned to
/// must be answered from the row.
#[test]
fn a_country_returns_with_the_address_it_came_from() {
    let db = db();
    let id = node(&db, 1);
    let facts = serde_json::json!({});
    let save = |source: &str| db.save_facts(id, &facts, "198.51.100.4", source).unwrap();
    let stored = || db.node(id).unwrap().unwrap().country;
    let (v4, v6) = ("198.51.100.4", "2001:db8::5");

    save(v4);
    db.set_country(id, "RU", v4).unwrap();
    assert!(save(v6), "an address never answered is asked about");
    db.set_country(id, "US", v6).unwrap();
    assert!(!save(v4), "the address before it is not asked about again");
    assert_eq!(stored(), "RU");
    assert!(!save(v6), "nor, after that, the one in between");
    assert_eq!(stored(), "US");

    // Addresses never answered pass through without displacing the last answer.
    assert!(save("203.0.113.9"));
    assert!(save("203.0.113.10"));
    assert!(!save(v6));
    assert_eq!(stored(), "US");
}

/// A hub before schema 5 looked every country up from `ip`. After the
/// upgrade a node whose source is still `ip` keeps its badge, and one whose
/// public interface address now takes precedence is asked about again.
#[test]
fn countries_stored_before_the_source_column_belong_to_the_connection_address() {
    let db = db();
    let (kept, moved) = (node(&db, 1), node(&db, 1));
    let facts = serde_json::json!({});
    for id in [kept, moved] {
        db.save_facts(id, &facts, "198.51.100.4", "198.51.100.4").unwrap();
        db.set_country(id, "SG", "198.51.100.4").unwrap();
    }
    {
        let conn = db.conn();
        conn.execute_batch("ALTER TABLE node DROP COLUMN country_ip").unwrap();
        migrate(&conn, 4).unwrap();
    }
    assert!(!db.save_facts(kept, &facts, "198.51.100.4", "198.51.100.4").unwrap());
    assert_eq!(db.node(kept).unwrap().unwrap().country, "SG");
    assert!(db.save_facts(moved, &facts, "198.51.100.4", "2001:db8::5").unwrap());
    assert_eq!(db.node(moved).unwrap().unwrap().country, "");
}

#[test]
fn traffic_survives_a_reboot_instead_of_resetting() {
    let db = db();
    let id = node(&db, 1);

    // The first report only establishes the baseline.
    let t = db.accumulate(id, "boot-a", (5_000, 3_000), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (0, 0));

    let t = db.accumulate(id, "boot-a", (9_000, 6_000), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (4_000, 3_000));

    // Reboot: a new boot_id with counters restarting near zero. The total must
    // not fall back to the fresh value, and the 700 bytes moved before the
    // first report are not booked, nothing having measured them.
    let t = db.accumulate(id, "boot-b", (700, 400), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (4_000, 3_000), "a reboot must not reset the total");

    // Counting resumes from the new baseline.
    let t = db.accumulate(id, "boot-b", (1_700, 900), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (5_000, 3_500));
    assert_eq!((t.month_rx, t.month_tx), (5_000, 3_500));
}

/// One install command pasted onto a second machine: both agents answer for
/// the same node and evict each other, so the hub sees two boot_ids
/// alternating, each with its own lifetime counter. Booking those would add
/// roughly 180 GB per swap to a total that only increases.
#[test]
fn two_machines_sharing_one_token_cannot_inflate_the_total() {
    let db = db();
    let id = node(&db, 1);
    let (a, b) = (100_000_000_000, 80_000_000_000); // two lifetime counters

    db.accumulate(id, "boot-a", (a, a), Local::now(), None).unwrap();
    let t = db.accumulate(id, "boot-a", (a + 1_000, a + 1_000), Local::now(), None).unwrap();
    assert_eq!(t.total_rx, 1_000, "the real machine's own traffic still counts");

    // Every swap presents a boot_id with no baseline, so every swap books
    // nothing.
    for round in 0..3 {
        db.accumulate(id, "boot-b", (b + round, b + round), Local::now(), None).unwrap();
        db.accumulate(id, "boot-a", (a + 1_000 + round, a + 1_000 + round), Local::now(), None).unwrap();
    }
    let t = db.all_traffic()[&id].clone();
    assert!(t.total_rx < 10_000, "six swaps booked {} bytes, not a lifetime counter", t.total_rx);
}

#[test]
fn a_shrinking_reading_re_aligns_instead_of_re_counting_history() {
    let db = db();
    let id = node(&db, 1);
    db.accumulate(id, "boot-a", (10_000, 10_000), Local::now(), None).unwrap();
    let t = db.accumulate(id, "boot-a", (12_000, 12_000), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (2_000, 2_000));

    // The same boot with a reduced reading: an interface included in the sum
    // has gone, so this is the remainder of the machine's history rather than
    // new bytes.
    let t = db.accumulate(id, "boot-a", (500, 500), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (2_000, 2_000));

    // Aligned to the smaller baseline, counting resumes from there.
    let t = db.accumulate(id, "boot-a", (900, 900), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (2_400, 2_400));

    // A new boot realigns identically, for the same reason: it has no baseline
    // either.
    let t = db.accumulate(id, "boot-b", (300, 300), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (2_400, 2_400));

    // One direction shrinking does not deprive the other of its increment.
    let t = db.accumulate(id, "boot-b", (100, 900), Local::now(), None).unwrap();
    assert_eq!((t.total_rx, t.total_tx), (2_400, 3_000));
}

/// The two counters that restart on their own schedules, against a total that
/// never does. Each derives from its own stored date, so a rollover must leave
/// the other untouched.
#[test]
fn day_and_month_restart_independently_while_the_total_keeps_climbing() {
    let db = db();
    let id = node(&db, 1);
    db.accumulate(id, "boot-a", (0, 0), Local::now(), None).unwrap();
    let t = db.accumulate(id, "boot-a", (8_000, 4_000), Local::now(), None).unwrap();
    assert_eq!((t.day_rx, t.day_tx), (8_000, 4_000));
    assert_eq!((t.month_rx, t.month_tx), (8_000, 4_000));

    // Midnight passes, forced through the stored date the rollover reads.
    db.conn().execute("UPDATE traffic SET day_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();
    let t = db.accumulate(id, "boot-a", (9_500, 4_600), Local::now(), None).unwrap();
    assert_eq!((t.day_rx, t.day_tx), (1_500, 600), "a new day counts only this report's delta");
    assert_eq!(t.month_rx, 9_500, "the month is not a day");
    assert_eq!(t.total_rx, 9_500, "and the total is neither");

    // The billing period then rolls over, partway through that same day.
    db.conn().execute("UPDATE traffic SET month_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();
    let t = db.accumulate(id, "boot-a", (10_000, 4_700), Local::now(), None).unwrap();
    assert_eq!((t.month_rx, t.month_tx), (500, 100), "a new period counts only this report's delta");
    assert_eq!((t.day_rx, t.day_tx), (2_000, 700), "the day carries on across a billing rollover");
    assert_eq!(t.total_rx, 10_000, "lifetime total is untouched by either rollover");
}

/// A reading is dated by its arrival, which for one held back over a boundary
/// is earlier than its booking. It must never take the row back into a period
/// already stamped on it, as the panel stamps the current one with a
/// correction. A later date still moves the period, as a changed reset day
/// requires, and a stamp later than today is not held to.
#[test]
fn a_reading_is_never_booked_into_a_period_already_left() {
    use chrono::TimeZone;
    let db = db();
    let id = node(&db, 20);
    let on = |d: u32| Local.with_ymd_and_hms(2026, 9, d, 12, 0, 0).unwrap();
    db.accumulate(id, "boot-a", (1_000, 0), on(24), None).unwrap();
    db.accumulate(id, "boot-a", (3_000, 0), on(24), None).unwrap();
    let t = db.accumulate(id, "boot-a", (3_500, 0), on(23), None).unwrap();
    assert_eq!(t.day_rx, 2_500, "a reading dated the day before does not restart today");
    assert_eq!(t.month_start, "2026-09-20");

    // The reset day moves to the 1st: the period now starts earlier, and a
    // reading dated after the stamp still switches to it.
    db.update_node(id, &NodePatch { traffic_reset_day: Some(1), ..Default::default() }).unwrap();
    let t = db.accumulate(id, "boot-a", (4_000, 0), on(24), None).unwrap();
    assert_eq!((t.month_start.as_str(), t.month_rx), ("2026-09-01", 500));

    // A correction stamps the current period; a reading from the one before,
    // held over the boundary, lands on top of it rather than discarding it.
    db.set_traffic(id, &TrafficPatch { month_rx: Some(10_000), ..Default::default() }).unwrap();
    let t = db.accumulate(id, "boot-a", (4_500, 0), Local::now() - chrono::Duration::days(40), None).unwrap();
    assert_eq!(t.month_rx, 10_500, "the correction survives a reading dated before it");

    // A clock a year ahead stamps its own day and period. Once it is stepped
    // back, the next reading returns the row to today.
    db.accumulate(id, "boot-a", (5_000, 0), Local::now() + chrono::Duration::days(365), None).unwrap();
    db.accumulate(id, "boot-a", (5_200, 0), Local::now(), None).unwrap();
    let t = db.all_traffic().remove(&id).unwrap();
    assert_eq!((t.day_rx, t.month_rx), (200, 200), "today reads what moved today, not zero");
}

/// The other half of the rollover: the counters restart on the node's next
/// report, so a node silent since before a boundary still holds the previous
/// period's bytes on disk. The read side must not return those.
#[test]
fn a_node_that_went_quiet_before_a_boundary_reads_as_zero_this_period() {
    let db = db();
    let id = node(&db, 1);
    db.accumulate(id, "boot-a", (0, 0), Local::now(), None).unwrap();
    db.accumulate(id, "boot-a", (8_000, 4_000), Local::now(), None).unwrap();
    assert_eq!(db.all_traffic()[&id].day_rx, 8_000, "still today, so it still counts");

    // Offline across both boundaries, with no report to restart either.
    db.conn()
        .execute("UPDATE traffic SET day_start='1999-01-01', month_start='1999-01-01' WHERE node_id=?1", [id])
        .unwrap();
    let t = db.all_traffic()[&id].clone();
    assert_eq!((t.day_rx, t.day_tx), (0, 0), "yesterday's bytes are not today's");
    assert_eq!((t.month_rx, t.month_tx), (0, 0), "last period's bytes are not this period's");
    assert_eq!(t.month_start, period_start(Local::now().date_naive(), 1).to_string());
    assert_eq!((t.total_rx, t.total_tx), (8_000, 4_000), "the lifetime total never resets");
}

/// Received 3, sent 5. "up" is the node's upload, which it sends.
#[test]
fn usage_is_counted_the_way_the_plan_meters_it() {
    let t = Traffic { month_rx: 3, month_tx: 5, ..Default::default() };
    assert_eq!(["sum", "up", "down", "max"].map(|mode| t.month_used(mode)), [8, 5, 3, 5]);
}

#[test]
fn period_start_handles_short_months_and_wraparound() {
    let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
    // Reset on the 15th, today the 20th: the current month.
    assert_eq!(period_start(d(2026, 3, 20), 15), d(2026, 3, 15));
    // The reset day itself counts as the start of the new period.
    assert_eq!(period_start(d(2026, 3, 15), 15), d(2026, 3, 15));
    // Before the reset day the period began in the previous month.
    assert_eq!(period_start(d(2026, 3, 10), 15), d(2026, 2, 15));
    // January rolls back into the previous year.
    assert_eq!(period_start(d(2026, 1, 10), 15), d(2025, 12, 15));
    // Day 31 in February clamps to the 28th; 2028 is a leap year.
    assert_eq!(period_start(d(2026, 2, 28), 31), d(2026, 2, 28));
    assert_eq!(period_start(d(2028, 2, 29), 31), d(2028, 2, 29));
}

#[test]
fn deleting_a_node_takes_its_data_with_it() {
    let db = db();
    let id = node(&db, 1);
    let probe = |nodes| PingTask {
        id: 0,
        name: "cm".into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes,
        ..Default::default()
    };
    let task = db.save_ping_task(&probe(vec![id])).unwrap();
    db.accumulate(id, "b", (10, 10), Local::now(), None).unwrap();
    db.insert_metric(id, 1, &serde_json::json!({"cpu": 1.0})).unwrap();
    db.insert_pings(id, &[(task, 1, 42)]).unwrap();
    db.delete_node(id).unwrap();
    assert!(db.node(id).unwrap().is_none());
    let pings = db
        .conn()
        .query_row("SELECT COUNT(*) FROM ping_record WHERE node_id = ?1", [id], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(pings, 0, "ping_record has no foreign key to cascade through");
    assert_eq!(db.metrics(id, Span::minutes(0, 60)).unwrap().len(), 0);
    assert!(!db.all_traffic().contains_key(&id));
    // Ticked in an editor opened before the delete: named, not a 500.
    let gone = db.save_ping_task(&probe(vec![id])).unwrap_err();
    let expected = format!("节点 {id} 不存在，可能已被删除");
    assert_eq!(
        gone.downcast_ref::<crate::Shown>().map(|s| s.0.as_str()),
        Some(expected.as_str()),
        "{gone:#}"
    );

    assert_ne!(node(&db, 1), id, "a deleted node's id is not issued again");
}

/// The mirror of the sweep above, on the other key of the same table. SQLite
/// reuses a deleted probe's id as well, and the chart selects on a node's
/// assignments, so the removed probe's samples would reappear under the new
/// probe's name with its timeouts folded into the new loss figure.
#[test]
fn deleting_a_probe_takes_its_history_with_it() {
    let db = db();
    let id = node(&db, 1);
    let probe = |name: &str| PingTask {
        id: 0,
        name: name.into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes: vec![id],
        ..Default::default()
    };
    let old = db.save_ping_task(&probe("tokyo")).unwrap();
    db.insert_pings(id, &[(old, 1, 999)]).unwrap();
    db.delete_ping_task(old).unwrap();
    let pings = db
        .conn()
        .query_row("SELECT COUNT(*) FROM ping_record WHERE task_id = ?1", [old], |r| r.get::<_, i64>(0))
        .unwrap();
    assert_eq!(pings, 0, "ping_record has no foreign key to cascade through");

    let fresh = db.save_ping_task(&probe("singapore")).unwrap();
    assert_ne!(fresh, old, "a deleted probe's id is not issued again");
}

/// Counted directly from the table rather than read back through
/// `ping_records`: that query filters on the node's assignments, so a row
/// written under a probe it does not have is invisible to it. An assertion
/// made through it therefore could not fail for the write this test exists to
/// prevent.
#[test]
fn a_result_for_a_probe_this_node_does_not_have_is_not_stored() {
    let db = db();
    let mine = node(&db, 1);
    let other = node(&db, 1);
    let rows = || db.conn().query_row("SELECT COUNT(*) FROM ping_record", [], |r| r.get::<_, i64>(0));
    let task = db
        .save_ping_task(&PingTask {
            id: 0,
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![mine],
            ..Default::default()
        })
        .unwrap();

    db.insert_pings(mine, &[(task, 1, 42)]).unwrap();
    assert_eq!(rows().unwrap(), 1, "the node the probe is assigned to files its own result");

    // A probe that exists but belongs to another node, and ids naming no probe
    // at all: what a node token can place on the wire.
    db.insert_pings(other, &[(task, 1, 42)]).unwrap();
    for invented in [7, 999_999, i64::from(i32::MAX) + 1] {
        db.insert_pings(mine, &[(invented, 1, 42)]).unwrap();
    }
    assert_eq!(rows().unwrap(), 1, "nothing else reaches the table");

    // Deleting the probe also ends its node's results, so one already in flight
    // cannot land after the sweep and stay behind under an id no probe holds.
    db.delete_ping_task(task).unwrap();
    db.insert_pings(mine, &[(task, 2, 42)]).unwrap();
    assert_eq!(rows().unwrap(), 0, "a late result for a deleted probe is dropped");
}

/// The strings in a `hello` come from an unvouched machine, and six of them go
/// straight into the frame pushed to the public page every two seconds, so
/// their length cannot be the node's to choose. `api` enforces the same bound
/// on the one string `agent_register` accepts.
#[test]
fn facts_from_an_unvouched_machine_cannot_choose_their_own_length() {
    let db = db();
    let id = node(&db, 1);
    db.save_facts(id, &serde_json::json!({"os": "A".repeat(10_000), "hostname": "x\u{7}y"}), "ip", "")
        .unwrap();
    let stored = db.node(id).unwrap().unwrap();
    assert_eq!(stored.os.chars().count(), 128);
    assert_eq!(stored.hostname, "xy", "control characters break the panel's rows");

    db.save_facts(id, &serde_json::json!({"os": "<img src=x onerror=\"a()\">'Debian'"}), "ip", "").unwrap();
    assert_eq!(db.node(id).unwrap().unwrap().os, "img src=x onerror=a()Debian", "no markup reaches a theme");
}

/// A correction must survive the node's return. `all_traffic` gates the month
/// figures on the period they were written for, and `accumulate` restarts the
/// counter when the stored period is stale, so a correction left under the
/// previous period would read as zero and then be discarded.
#[test]
fn a_month_correction_is_stamped_with_the_period_it_was_made_in() {
    let db = db();
    let id = node(&db, 1);
    db.accumulate(id, "boot-a", (0, 0), Local::now(), None).unwrap();
    // A node silent since before its reset day still holds the old period.
    db.conn().execute("UPDATE traffic SET month_start='1999-01-01' WHERE node_id=?1", [id]).unwrap();

    db.set_traffic(
        id,
        &TrafficPatch {
            total_rx: Some(4_000),
            total_tx: Some(2_000),
            month_rx: Some(300),
            month_tx: Some(100),
        },
    )
    .unwrap();
    let t = db.all_traffic().remove(&id).unwrap();
    assert_eq!((t.month_rx, t.month_tx), (300, 100), "the correction reads back as this period's");

    let t = db.accumulate(id, "boot-a", (500, 50), Local::now(), None).unwrap();
    assert_eq!((t.month_rx, t.month_tx), (800, 150), "and the next report adds to it");
    assert_eq!((t.total_rx, t.total_tx), (4_500, 2_050));
}

#[test]
fn partial_edits_keep_other_settings_and_live_counters() {
    let db = db();
    let id = node(&db, 1);
    let patch = |v| serde_json::from_value::<NodePatch>(v).unwrap();
    db.update_node(
        id,
        &patch(serde_json::json!({"public":false,"remark":"private","public_remark":"CN2 GIA","expires_at":"2030-01-01"})),
    )
    .unwrap();
    db.update_node(id, &patch(serde_json::json!({"price":20}))).unwrap();
    let n = db.node(id).unwrap().unwrap();
    assert!(!n.public);
    assert_eq!(n.remark, "private");
    assert_eq!(n.public_remark, "CN2 GIA");
    assert_eq!(n.expires_at.as_deref(), Some("2030-01-01"));
    db.update_node(id, &patch(serde_json::json!({"price":0,"expires_at":null}))).unwrap();
    let n = db.node(id).unwrap().unwrap();
    assert_eq!(n.price, 0.0);
    assert_eq!(n.expires_at, None);

    db.accumulate(id, "boot", (0, 0), Local::now(), None).unwrap();
    db.accumulate(id, "boot", (120_000, 10_000), Local::now(), None).unwrap();
    db.set_traffic(id, &TrafficPatch { month_tx: Some(3_000), ..Default::default() }).unwrap();
    let t = db.all_traffic().remove(&id).unwrap();
    assert_eq!((t.total_rx, t.total_tx, t.month_rx, t.month_tx), (120_000, 10_000, 120_000, 3_000));

    db.update_node(id, &patch(serde_json::json!({"traffic_reset_day":2}))).unwrap();
    db.set_traffic(id, &TrafficPatch { month_rx: Some(7_000), ..Default::default() }).unwrap();
    let t = db.all_traffic().remove(&id).unwrap();
    assert_eq!((t.month_rx, t.month_tx), (7_000, 0));
    // Correcting only a lifetime total cannot revive the previous month's
    // bytes.
    db.conn()
        .execute("UPDATE traffic SET month_start='1999-01-01',month_tx=999 WHERE node_id=?1", [id])
        .unwrap();
    db.set_traffic(id, &TrafficPatch { total_rx: Some(130_000), ..Default::default() }).unwrap();
    assert_eq!(db.all_traffic()[&id].month_tx, 0);
}

#[test]
fn a_token_is_readable_and_rotation_retires_the_old_one() {
    let db = db();
    let id = db.create_node(&Node { name: "n".into(), ..Default::default() }, "first-token").unwrap();

    // Readable, so the panel can display the install command without issuing a
    // new token.
    assert_eq!(db.node(id).unwrap().unwrap().token, "first-token");
    assert_eq!(db.node_by_token("first-token").unwrap(), Some(id));

    db.reset_token(id, "second-token").unwrap();
    assert_eq!(db.node(id).unwrap().unwrap().token, "second-token");
    assert_eq!(db.node_by_token("second-token").unwrap(), Some(id));
    assert_eq!(db.node_by_token("first-token").unwrap(), None, "the old token stops working");
}

#[test]
fn nodes_can_be_reordered_atomically() {
    let db = db();
    let (a, b, c) = (node(&db, 1), node(&db, 1), node(&db, 1));
    let order = || db.nodes().unwrap().iter().map(|n| n.id).collect::<Vec<_>>();
    db.reorder_nodes(&[c, a, b]).unwrap();
    assert_eq!(order(), vec![c, a, b]);

    // Every rejected input leaves the existing order intact. The partial list
    // matters most: a stale tab would otherwise renumber around a node it never
    // saw.
    assert!(db.reorder_nodes(&[a, a, c]).is_err(), "duplicates");
    assert!(db.reorder_nodes(&[a, b]).is_err(), "a node left out");
    assert!(db.reorder_nodes(&[a, b, 9999]).is_err(), "an id that is not a node");
    assert_eq!(order(), vec![c, a, b]);
    // A node added afterwards goes to the end rather than wherever sort 0
    // places it.
    let d = node(&db, 1);
    assert_eq!(db.nodes().unwrap().iter().map(|n| n.id).collect::<Vec<_>>(), vec![c, a, b, d]);
}

/// Themes draw probes in the order they first appear in the rows, so the rows
/// follow the panel's order even when a later probe alone answered in the
/// window's first bucket.
#[test]
fn a_probe_chart_follows_the_panel_order() {
    let db = db();
    let id = node(&db, 1);
    let probe = |name: &str| {
        db.save_ping_task(&PingTask {
            name: name.into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![id],
            ..Default::default()
        })
        .unwrap()
    };
    let (a, b) = (probe("a"), probe("b"));
    db.reorder_ping_tasks(&[b, a]).unwrap();
    let c = probe("c");
    let listed: Vec<_> = db.ping_tasks().unwrap().iter().map(|t| t.id).collect();
    assert_eq!(listed, vec![b, a, c], "a new probe starts at the end");

    db.insert_pings(id, &[(a, 0, 10), (a, 60, 10), (b, 60, 20), (c, 60, 30)]).unwrap();
    let rows = db.ping_records(id, Span::minutes(0, 60)).unwrap().0;
    let drawn: Vec<_> =
        rows.iter().map(|r| (r["task_id"].as_i64().unwrap(), r["ts"].as_i64().unwrap())).collect();
    assert_eq!(drawn, vec![(b, 60), (a, 0), (a, 60), (c, 60)]);
}

#[test]
fn prune_drops_history_but_never_traffic_totals() {
    let db = db();
    let id = node(&db, 1);
    db.accumulate(id, "b", (100, 100), Local::now(), None).unwrap();
    db.accumulate(id, "b", (900, 900), Local::now(), None).unwrap();
    let old = Utc::now().timestamp() - 40 * 86_400;
    db.insert_metric(id, old, &serde_json::json!({"cpu": 1.0})).unwrap();
    db.insert_metric(id, Utc::now().timestamp(), &serde_json::json!({"cpu": 2.0})).unwrap();

    db.prune(30).unwrap();
    assert_eq!(db.metrics(id, Span::minutes(0, 60)).unwrap().len(), 2, "unfolded minutes are kept");
    db.roll_up(Utc::now().timestamp(), 30).unwrap();
    db.prune(30).unwrap();
    assert_eq!(db.metrics(id, Span::minutes(0, 60)).unwrap().len(), 1);
    assert_eq!(db.all_traffic()[&id].total_rx, 800);
}

/// The rekeying in `open()`: rows must survive it, and the chart's query must
/// emerge able to seek. A migration that leaves every row on the old key fails
/// silently, and stays silent while the query it exists for scans a node's
/// entire history.
#[test]
fn rekeying_ping_record_keeps_the_rows_and_lets_the_chart_query_seek() {
    let file = std::env::temp_dir().join(format!("monitor-rekey-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let path = file.to_str().unwrap();

    // A database as an older hub left it.
    let old = Connection::open(path).unwrap();
    old.execute_batch(
        "CREATE TABLE ping_record (
               node_id INTEGER NOT NULL, task_id INTEGER NOT NULL,
               ts INTEGER NOT NULL, latency INTEGER NOT NULL,
               PRIMARY KEY (node_id, task_id, ts)
             ) WITHOUT ROWID;
             INSERT INTO ping_record VALUES (1,7,100,12),(1,8,100,34),(1,7,200,56),(2,7,100,78);",
    )
    .unwrap();
    drop(old);

    let db = Db::open(path).unwrap();
    let conn = db.conn();
    let rows: Vec<(i64, i64, i64, i64)> = conn
        .prepare("SELECT node_id, task_id, ts, latency FROM ping_record ORDER BY node_id, ts, task_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, vec![(1, 7, 100, 12), (1, 8, 100, 34), (1, 7, 200, 56), (2, 7, 100, 78)]);

    // Without the timestamp second in the key the plan stops at `node_id=?`
    // and scans everything beneath it, and the fold in `ping_records` requires
    // rows in time order, which only the seek provides without a sorter.
    let plan: String = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {PING_ROWS}"))
        .unwrap()
        .query_map(params![1, 0, 60], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(" | ");
    assert!(plan.contains("node_id=? AND ts>?"), "the window has to be a seek, not a scan: {plan}");
    assert!(!plan.contains("ORDER BY"), "the time order has to come off the key, not a sorter: {plan}");

    // Opening again must not rebuild a table that is already correct.
    drop(conn);
    drop(db);
    assert!(Db::open(path).is_ok());
    let _ = std::fs::remove_file(&file);
}

/// Every row of every table, comparable across two opens of one file.
fn dump(conn: &Connection) -> Vec<String> {
    let mut rows = Vec::new();
    for table in TABLES {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        if !exists {
            continue;
        }
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let width = stmt.column_count();
        let read = |r: &rusqlite::Row| (0..width).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect();
        let mut found: Vec<String> = stmt
            .query_map([], |r| read(r))
            .unwrap()
            .map(|row: rusqlite::Result<Vec<_>>| format!("{table} {:?}", row.unwrap()))
            .collect();
        found.sort();
        rows.extend(found);
    }
    rows
}

/// A file as the oldest release left it, one row in every table.
fn release_file(path: &str) {
    let old = Connection::open(path).unwrap();
    old.execute_batch(include_str!("../testdata/schema-v1.0.0.sql")).unwrap();
    old.execute_batch(
        "INSERT INTO setting VALUES ('site', 'https://hub.example.com');
             INSERT INTO node (id, name, token, ip, country, created_at)
               VALUES (1, 'n', 't', '198.51.100.4', 'US', 1);
             INSERT INTO traffic (node_id, total_rx) VALUES (1, 5000);
             INSERT INTO metric VALUES (1, 60, 12.5, 100, 0, 0, 0, 0, 0, 0, 0);
             INSERT INTO ping_task (id, name, target) VALUES (1, 'cm', '1.1.1.1:443');
             INSERT INTO ping_node VALUES (1, 1);
             INSERT INTO ping_record VALUES (1, 1, 60, 42);
             INSERT INTO session VALUES ('h', 9999999999);
             PRAGMA user_version = 3;",
    )
    .unwrap();
}

/// v1.0.0's schema, opened by this build through the startup path. Fails on
/// a column `SCHEMA` gained without a migration or the reverse, on an index
/// in `SCHEMA` over a column only a migration adds, and on a migration that
/// changes the data when it runs a second time.
#[test]
fn an_upgraded_release_matches_a_fresh_database() {
    let scratch = Scratch::new();
    release_file(&scratch.0);
    let db = Db::open(&scratch.0).unwrap();
    let fresh = Db::open(":memory:").unwrap();
    for table in TABLES {
        assert_eq!(
            columns_of(&db.conn(), table).unwrap(),
            columns_of(&fresh.conn(), table).unwrap(),
            "{table}"
        );
    }
    let upgraded = dump(&db.conn());
    assert_eq!(
        upgraded.len(),
        PRE_RESOURCE_TABLES.len() + 2 + ID_TABLES.len(),
        "every old row survives, admin is seeded, and the reset marker is recorded: {upgraded:#?}"
    );

    // An earlier build opening the file stamps its own version, so the next
    // upgrade runs every migration again.
    db.conn().execute_batch("PRAGMA user_version = 3").unwrap();
    drop(db);
    let db = Db::open(&scratch.0).unwrap();
    assert_eq!(dump(&db.conn()), upgraded, "a second run changes nothing");
    let version: i64 = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

/// The trigger fails the UPDATE in `migrate_to_5` after `migrate_to_4` has
/// added its columns: a failure part-way through an upgrade.
#[test]
fn a_failed_upgrade_leaves_the_file_as_it_was() {
    let scratch = Scratch::new();
    release_file(&scratch.0);
    let state = |c: &Connection| {
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        (version, columns_of(c, "node").unwrap(), dump(c))
    };
    let before = {
        let c = Connection::open(&scratch.0).unwrap();
        c.execute_batch(
            "CREATE TRIGGER fail BEFORE UPDATE ON node BEGIN SELECT RAISE(ABORT, 'disk full'); END",
        )
        .unwrap();
        state(&c)
    };
    assert!(Db::open(&scratch.0).is_err());
    assert_eq!(state(&Connection::open(&scratch.0).unwrap()), before, "no step of the upgrade remains");
}

/// Dropping `metric.load1` under a database in service. The column is
/// `NOT NULL` with no default, so a migration that silently failed to run
/// would not merely leave a stale column: it would prevent every history row
/// from being written.
#[test]
fn dropping_load1_keeps_the_history_and_lets_new_rows_in() {
    let file = std::env::temp_dir().join(format!("monitor-load1-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let path = file.to_str().unwrap();

    // A database as a hub predating this build left it: one metric row
    // carrying a load average, stamped with the schema version of the time.
    let old = Connection::open(path).unwrap();
    old.execute_batch(
        "CREATE TABLE metric (
               node_id INTEGER NOT NULL, ts INTEGER NOT NULL,
               cpu REAL NOT NULL, load1 REAL NOT NULL,
               mem_used INTEGER NOT NULL, swap_used INTEGER NOT NULL, disk_used INTEGER NOT NULL,
               net_rx INTEGER NOT NULL, net_tx INTEGER NOT NULL,
               tcp INTEGER NOT NULL, udp INTEGER NOT NULL, procs INTEGER NOT NULL,
               PRIMARY KEY (node_id, ts)
             ) WITHOUT ROWID;
             INSERT INTO metric VALUES (1,60,12.5,0.75,100,0,0,0,0,0,0,0);
             PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(old);

    let db = Db::open(path).unwrap();
    assert!(!schema_mentions(&db.conn(), "metric", "load1").unwrap(), "the column has to be gone");
    // The row remains, along with everything else it carried.
    let kept = &db.metrics(1, Span::minutes(0, 60)).unwrap()[0];
    assert_eq!((kept["ts"].as_i64(), kept["cpu"].as_f64()), (Some(60), Some(12.5)));
    // The shape this build inserts now fits the table.
    db.insert_metric(1, 120, &serde_json::json!({"cpu": 2.0, "load": [0.5, 0.4, 0.3]})).unwrap();
    assert_eq!(db.metrics(1, Span::minutes(0, 60)).unwrap().len(), 2);

    // Opening again must not attempt to drop a column already removed.
    drop(db);
    assert!(Db::open(path).is_ok());
    let _ = std::fs::remove_file(&file);
}

/// Removing a node from a probe must remove the probe from that node's chart.
/// `ping_record` carries no foreign key to the assignment that produced it, so
/// the rows outlive it until retention; the window query is what must stop
/// drawing them, and immediately rather than at the next hourly sweep.
#[test]
fn a_probe_taken_off_a_node_stops_appearing_in_its_history() {
    let db = db();
    let id = node(&db, 1);
    let probe = |nodes: Vec<i64>, task| {
        db.save_ping_task(&PingTask {
            id: task,
            name: "cm".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes,
            ..Default::default()
        })
        .unwrap()
    };
    let task = probe(vec![id], 0);
    db.insert_pings(id, &[(task, 100, 42)]).unwrap();
    assert_eq!(db.ping_records(id, Span::minutes(0, 60)).unwrap().0.len(), 1, "an assigned probe draws");

    probe(vec![], task);
    assert!(db.ping_records(id, Span::minutes(0, 60)).unwrap().0.is_empty(), "an unassigned one does not");

    // The rows remain: reassigning restores the history rather than starting
    // over.
    probe(vec![id], task);
    assert_eq!(
        db.ping_records(id, Span::minutes(0, 60)).unwrap().0.len(),
        1,
        "and it comes back with its history"
    );

    // The names accompany those samples and follow the same filter: a probe
    // name is operator-supplied text that routinely carries a hostname or a
    // customer.
    assert_eq!(db.ping_task_names(id).unwrap()[&task.to_string()], "cm");
    let other = node(&db, 1);
    assert!(
        db.ping_task_names(other).unwrap().as_object().is_some_and(|m| m.is_empty()),
        "a node the probe was never assigned to must not learn its name"
    );
}

#[test]
fn ping_tasks_round_trip_with_their_node_assignments() {
    let db = db();
    let (a, b) = (node(&db, 1), node(&db, 1));
    let id = db
        .save_ping_task(&PingTask {
            id: 0,
            name: "cf".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![a, b],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(db.ping_tasks_for(a).unwrap().len(), 1);

    // Reassigning to one node must drop the other's copy.
    db.save_ping_task(&PingTask {
        id,
        name: "cf".into(),
        target: "1.1.1.1:443".into(),
        interval: 30,
        nodes: vec![a],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(db.ping_tasks_for(b).unwrap().len(), 0);
    assert_eq!(db.ping_tasks().unwrap()[0].interval, 30);
}

#[test]
fn an_auto_joining_probe_is_assigned_to_nodes_created_after_it() {
    let db = db();
    let existing = node(&db, 1);
    let probe = |auto_join| PingTask {
        id: 0,
        name: "p".into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes: vec![],
        auto_join,
        ..Default::default()
    };
    let joining = db.save_ping_task(&probe(true)).unwrap();
    db.save_ping_task(&probe(false)).unwrap();
    assert!(db.ping_tasks_for(existing).unwrap().is_empty(), "existing nodes follow the list alone");

    let added = node(&db, 1);
    let assigned: Vec<i64> =
        db.ping_tasks_for(added).unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
    assert_eq!(assigned, [joining]);
    assert!(db.ping_tasks().unwrap()[0].auto_join);

    // A new node takes every auto-joining probe at once, so their count is
    // held to what one agent runs.
    for _ in 1..Db::MAX_PROBES_PER_NODE {
        db.save_ping_task(&probe(true)).unwrap();
    }
    assert!(db.save_ping_task(&probe(true)).is_err());
    assert_eq!(db.ping_tasks().unwrap().len() as i64, Db::MAX_PROBES_PER_NODE + 1);
}

/// The editor's list is a snapshot, while `create_node` assigns auto-joining
/// probes whenever a node registers.
#[test]
fn an_edit_applies_only_the_assignments_it_changed() {
    let db = db();
    let (a, b) = (node(&db, 1), node(&db, 1));
    let save = |id, nodes: Vec<i64>, base: Vec<i64>| {
        db.save_ping_task(&PingTask {
            id,
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes,
            auto_join: true,
            base: Some(base),
        })
    };
    let assigned = |id| {
        let mut nodes = db.ping_tasks().unwrap().into_iter().find(|t| t.id == id).unwrap().nodes;
        nodes.sort_unstable();
        nodes
    };
    let id = save(0, vec![a], vec![]).unwrap();
    let joined = node(&db, 1);

    // Opened before `joined` existed, so it is in neither list and stays.
    save(id, vec![a, b], vec![a]).unwrap();
    assert_eq!(assigned(id), [a, b, joined]);
    save(id, vec![b], vec![a, b]).unwrap();
    assert_eq!(assigned(id), [b, joined]);

    // Ticked after joining on its own: already assigned, not an error.
    save(id, vec![b, joined], vec![b]).unwrap();
    assert_eq!(assigned(id), [b, joined]);
    // OR IGNORE leaves the foreign key in force.
    assert!(save(id, vec![b, joined, 9999], vec![b, joined]).is_err(), "an id that is not a node");
    assert_eq!(assigned(id), [b, joined]);

    // A node deleted since the editor opened is in both lists and untouched.
    db.delete_node(b).unwrap();
    save(id, vec![b, joined], vec![b, joined]).unwrap();

    db.delete_ping_task(id).unwrap();
    assert!(save(id, vec![], vec![]).is_err(), "a deleted probe is not reported as saved");
    assert!(db.ping_tasks().unwrap().is_empty());
}

/// The agent caps the probe list it will run and drops the remainder with
/// nothing but a line in its own journal. The hub knows the total, so the hub
/// issues the refusal; otherwise the panel lists probes that never ran and
/// charts that stay empty, with the only record on the node.
#[test]
fn a_node_cannot_be_given_more_probes_than_the_agent_will_run() {
    let db = db();
    let id = node(&db, 1);
    let save = |task: i64, nodes: Vec<i64>| {
        db.save_ping_task(&PingTask {
            id: task,
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes,
            ..Default::default()
        })
    };
    for _ in 0..Db::MAX_PROBES_PER_NODE {
        save(0, vec![id]).unwrap();
    }
    assert_eq!(db.ping_tasks_for(id).unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);

    let refused = save(0, vec![id]).expect_err("one past the cap must be refused");
    assert!(refused.to_string().contains("探测任务"), "{refused}");
    // Rolled back entirely: the probe must not survive its assignment being
    // rejected, or the panel accumulates one that never runs.
    assert_eq!(db.ping_tasks().unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);
    assert_eq!(db.ping_tasks_for(id).unwrap().len() as i64, Db::MAX_PROBES_PER_NODE);

    // Editing an existing probe does not count as adding one.
    let first = db.ping_tasks().unwrap()[0].id;
    save(first, vec![id]).expect("an existing probe can still be edited at the cap");
}

#[tokio::test(flavor = "current_thread")]
async fn a_node_request_waiting_on_database_does_not_block_runtime() {
    let app = std::sync::Arc::new(crate::App::for_test(db()));
    let locked = app.clone();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let holder = std::thread::spawn(move || {
        let _connection = locked.db.conn();
        ready.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
    });
    waiting.await.unwrap();
    let request = tokio::spawn(crate::api::nodes(axum::extract::State(app), axum::http::HeaderMap::new()));
    let started = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert!(started.elapsed() < std::time::Duration::from_millis(100));
    assert_eq!(request.await.unwrap().status(), axum::http::StatusCode::OK);
    holder.join().unwrap();
}

/// One worker, as on a one-core hub: a task waiting for the writer must not
/// keep it, or the timer below would never fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn waiting_for_the_writer_leaves_the_runtime_running() {
    let db = std::sync::Arc::new(db());
    let (locked, taken) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        let holder = &db;
        s.spawn(move || {
            let _conn = holder.conn();
            locked.send(()).unwrap();
            let _ = held.recv();
        });
        taken.recv().unwrap();
        let db = db.clone();
        tokio::spawn(async move { db.get("site_name") });
        let (fired, ticks) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            fired.send(()).unwrap();
        });
        let outcome = ticks.recv_timeout(std::time::Duration::from_secs(5));
        drop(release);
        assert!(outcome.is_ok(), "a timer must fire while a task waits for the writer");
    });
}

/// The data page's counts must not queue behind a history chart holding
/// the reader, nor hold it while the public page's charts wait.
#[test]
fn stats_do_not_take_the_charts_reader() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let chart = db.reader.as_ref().unwrap().lock().unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let outcome = std::thread::scope(|s| {
        s.spawn(|| done.send(db.stats().is_ok()).unwrap());
        let outcome = finished.recv_timeout(std::time::Duration::from_secs(5));
        drop(chart);
        outcome
    });
    assert_eq!(outcome, Ok(true), "stats must not wait for the reader");
}

/// The history charts read through their own connection, so a scan never
/// holds up the writer: a week of eight 10-second probes is 486 ms, which
/// every agent report would otherwise wait out.
#[test]
fn history_is_read_while_the_writer_is_held() {
    let scratch = Scratch::new();
    let db = std::sync::Arc::new(Db::open(&scratch.0).unwrap());
    let id = db.create_node(&Node { name: "n".into(), ..Default::default() }, "token").unwrap();
    let task = db
        .save_ping_task(&PingTask {
            name: "probe".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![id],
            ..Default::default()
        })
        .unwrap();
    db.insert_metric(id, 60, &serde_json::json!({"cpu": 1.0})).unwrap();
    db.insert_pings(id, &[(task, 60, 42)]).unwrap();

    let writer = db.conn();
    let (send, receive) = std::sync::mpsc::channel();
    let reading = std::thread::spawn({
        let db = db.clone();
        move || {
            let span = Span::minutes(0, 60);
            let (metrics, pings) = (db.metrics(id, span).unwrap(), db.ping_records(id, span).unwrap().0);
            send.send((metrics, pings, db.ping_task_names(id).unwrap())).unwrap();
        }
    });
    // Bounded, so a read that does wait fails here rather than hanging the
    // test; released below, it then finishes.
    let read = receive.recv_timeout(std::time::Duration::from_secs(5));
    drop(writer);
    reading.join().unwrap();
    let (metrics, pings, names) = read.expect("history is read while the writer is held");
    assert_eq!((metrics.len(), pings.len()), (1, 1), "and it sees what the writer committed");
    assert_eq!(names[&task.to_string()], "probe");
}

/// Chart reads kept back to back leave no commit an idle reader to
/// checkpoint past, so the reader checkpoints for them and the WAL stays
/// bounded however long they continue. Without that, these writes would
/// grow it to 85 MB.
#[test]
fn chart_reads_back_to_back_do_not_grow_the_wal() {
    let scratch = Scratch::new();
    let db = std::sync::Arc::new(Db::open(&scratch.0).unwrap());
    let id = db.create_node(&Node { name: "n".into(), ..Default::default() }, "token").unwrap();
    let task = db
        .save_ping_task(&PingTask {
            name: "probe".into(),
            target: "1.1.1.1:443".into(),
            interval: 10,
            nodes: vec![id],
            ..Default::default()
        })
        .unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let (db, stop) = (db.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    db.ping_records(id, Span::minutes(0, 60)).unwrap();
                }
            })
        })
        .collect();
    let wal = format!("{}-wal", scratch.0);
    let mut largest = 0;
    for ts in 0..20_000 {
        db.insert_pings(id, &[(task, ts * 10, 40)]).unwrap();
        largest = largest.max(bytes_of(&wal));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    readers.into_iter().for_each(|r| r.join().unwrap());
    assert!(largest < 16 << 20, "the WAL reached {largest} bytes");
}

/// A closed database is one file again, as before the reader existed, so a
/// copy of the file taken with the hub stopped holds every row.
#[test]
fn a_closed_database_leaves_no_wal_behind() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let id = db.create_node(&Node { name: "n".into(), ..Default::default() }, "token").unwrap();
    db.insert_metric(id, 60, &serde_json::json!({"cpu": 1.0})).unwrap();
    // The reader joins the WAL on its first read, not when it opens.
    db.metrics(id, Span::minutes(0, 60)).unwrap();
    drop(db);
    assert!(!std::path::Path::new(&format!("{}-wal", scratch.0)).exists());
}

#[test]
fn version_twenty_gains_upstream_history_without_changing_resource_identity() {
    let db = db();
    let id = db.create_user(&user_draft("migration-user")).unwrap().unwrap();
    let uuid = db.user(id).unwrap().unwrap().uuid;
    {
        let conn = db.conn();
        conn.execute_batch(
            "DROP TABLE metric_hour; DROP TABLE ping_hour;
                 ALTER TABLE metric DROP COLUMN cpu_max;
                 ALTER TABLE node DROP COLUMN public_remark;
                 ALTER TABLE node DROP COLUMN ip_other;
                 PRAGMA user_version = 20;",
        )
        .unwrap();
        migrate(&conn, 20).unwrap();
        assert!(columns_of(&conn, "metric").unwrap().contains("cpu_max"));
        assert!(columns_of(&conn, "node").unwrap().contains("public_remark"));
        assert!(columns_of(&conn, "node").unwrap().contains("ip_other"));
        for table in HOURLY_TABLES {
            assert!(!columns_of(&conn, table).unwrap().is_empty());
        }
        migrate(&conn, SCHEMA_VERSION).unwrap();
    }
    assert_eq!(db.user(id).unwrap().unwrap().uuid, uuid);
}

/// A machine that rebooted while the hub was not hearing from it books its
/// counters since boot; a second machine on the token, booted before the
/// last report, and a change of interfaces alone do not.
#[test]
fn a_reboot_after_the_node_was_last_seen_books_the_counters_since_boot() {
    let db = db();
    let id = node(&db, 1);
    let now = Local::now();
    let total = |epoch: &str, counters: i64, booted: i64| {
        db.accumulate(id, epoch, (counters, counters), now, Some(booted)).unwrap().total_rx
    };
    assert_eq!(total("k1/a", 500_000_000_000, 0), 0, "a first reading is a baseline");
    db.touch_seen(id, 1_000, &serde_json::Value::Null).unwrap();
    assert_eq!(total("k1/b", 600_000_000_000, 2_000), 0, "the interfaces changed, the kernel did not");
    assert_eq!(total("k2/b", 80_000_000_000, 900), 0, "booted before the node was last seen");
    assert_eq!(total("k3/b", 7_000, 2_000), 7_000, "booted after it, from zero");
    assert_eq!(total("k3/b", 7_500, 2_000), 7_500);
}

/// A database from a release without high marks: the newest node and probe
/// deleted before anything is created keep their ids out of circulation.
#[test]
fn an_upgraded_database_does_not_reissue_the_newest_ids() {
    let file = std::env::temp_dir().join(format!("monitor-marks-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let path = file.to_str().unwrap();
    let db = Db::open(path).unwrap();
    let (_, newest) = (node(&db, 1), node(&db, 1));
    let probe = |name: &str| PingTask {
        name: name.into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes: vec![newest],
        ..Default::default()
    };
    let task = db.save_ping_task(&probe("tokyo")).unwrap();
    // As the release before the marks left it.
    db.conn().execute("DELETE FROM setting WHERE key LIKE '%_id_issued'", []).unwrap();
    drop(db);

    let db = Db::open(path).unwrap();
    db.delete_ping_task(task).unwrap();
    db.delete_node(newest).unwrap();
    assert!(node(&db, 1) > newest, "the newest node's id is issued again");
    assert!(db.save_ping_task(&probe("osaka")).is_err(), "the probe names a deleted node");
    let other = node(&db, 1);
    let fresh = db.save_ping_task(&PingTask { nodes: vec![other], ..probe("osaka") }).unwrap();
    assert!(fresh > task, "the newest probe's id is issued again");
    drop(db);
    std::fs::remove_file(&file).unwrap();
}

/// An earlier build deleting a node or a probe runs only its own statements,
/// those of v1.3.1 below, and knows nothing of `ping_hour`. Rows it leaves
/// would stay behind under an id no node or probe holds, once the newer
/// build is back.
#[test]
fn an_earlier_builds_deletes_clear_the_hourly_probe_rows() {
    let db = db();
    let (kept_node, gone_node) = (node(&db, 1), node(&db, 1));
    let probe = || PingTask {
        id: 0,
        name: "p".into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes: vec![kept_node, gone_node],
        ..Default::default()
    };
    let (kept_task, gone_task) = (db.save_ping_task(&probe()).unwrap(), db.save_ping_task(&probe()).unwrap());
    let conn = db.conn();
    for n in [kept_node, gone_node] {
        for t in [kept_task, gone_task] {
            conn.execute("INSERT INTO ping_hour VALUES (?1, ?2, 3600, 60, 0, 42, 40, 44)", params![n, t])
                .unwrap();
        }
    }
    conn.execute_batch(&format!(
            "DELETE FROM ping_record WHERE node_id = {gone_node}; DELETE FROM node WHERE id = {gone_node};
             DELETE FROM ping_record WHERE task_id = {gone_task}; DELETE FROM ping_task WHERE id = {gone_task};"
        ))
        .unwrap();
    let left: Vec<(i64, i64)> = conn
        .prepare("SELECT node_id, task_id FROM ping_hour")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(left, [(kept_node, kept_task)]);
}

/// `ping_hour` refers to `ping_task`, so a result filed under a probe that
/// no longer exists would fail the fold of its hour on every pass.
#[test]
fn a_result_left_by_a_deleted_probe_does_not_stop_the_rollup() {
    let db = db();
    let id = node(&db, 1);
    let now = Utc::now().timestamp();
    let hour = (now - 3 * 3_600).div_euclid(3_600) * 3_600;
    db.conn().execute("INSERT INTO ping_record VALUES (?1, 999, ?2, 42)", params![id, hour]).unwrap();
    db.insert_metric(id, hour, &serde_json::json!({"cpu": 1.0})).unwrap();
    assert!(db.roll_up(now, 1).unwrap() > 0);
    let span = Span { since: hour, step: 3_600, hourly: true };
    assert_eq!(db.metrics(id, span).unwrap().len(), 1, "the hour is folded");
}

/// The address seen in the other family arrives on its own connection, in no
/// fixed order against the hello, and must not be wiped by it.
#[test]
fn the_address_seen_in_the_other_family_survives_a_hello() {
    let db = db();
    let id = node(&db, 1);
    db.set_ip_other(id, "2001:db8:1::9").unwrap();
    db.save_facts(
        id,
        &serde_json::json!({"ipv4": "10.0.0.5", "ipv6": "fd00::5"}),
        "203.0.113.9",
        "203.0.113.9",
    )
    .unwrap();
    let stored = db.node(id).unwrap().unwrap();
    assert_eq!((stored.ip.as_str(), stored.ip_other.as_str()), ("203.0.113.9", "2001:db8:1::9"));
}

/// The hourly tier draws what the minute rows would. At an hour per point
/// every figure matches; at several hours per point the integer means differ
/// by the truncation of each hour's, the peaks, ranges and losses still
/// match, and the probe median is the weighted one, inside its range.
/// Checked while the last hour is still minute rows and again once folded.
#[test]
fn the_hourly_tier_draws_what_the_minute_rows_would() {
    let db = db();
    let id = node(&db, 1);
    let probe = |name: &str| {
        db.save_ping_task(&PingTask {
            id: 0,
            name: name.into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes: vec![id],
            ..Default::default()
        })
        .unwrap()
    };
    let (a, b) = (probe("a"), probe("b"));
    // Six hours whose values vary within each hour, with minutes missing so
    // the hours carry different weights, and a probe that loses rounds.
    let start = 472_224 * 3_600;
    // Per hour, the columns the official themes do not draw, to check the
    // tier keeps them.
    let mut kept: [Vec<[i64; 4]>; 6] = Default::default();
    for m in (0..6 * 60).filter(|m| m % 17 != 3 && !(100..130).contains(m)) {
        let ts = start + m * 60;
        let unreturned = [2_000_000 + m * 331, 10 + m % 4, m % 3, 120 + m % 9];
        kept[(m / 60) as usize].push(unreturned);
        let [swap_used, tcp, udp, procs] = unreturned;
        let sample = serde_json::json!({"cpu": (m % 7) as f64 * 1.5, "mem_used": 1_000_000 + m * 997,
                "disk_used": 5_000_000 + m, "net_rx": 1_000 + m * 13, "net_tx": 500 + m * 7,
                "net_rx_max": 3_000 + m % 50 * 40, "cpu_max": (m % 7) as f64 * 1.5 + (m % 11) as f64,
                "swap_used": swap_used, "tcp": tcp, "udp": udp, "procs": procs});
        db.insert_metric(id, ts, &sample).unwrap();
        db.insert_pings(
            id,
            &[(a, ts + 5, 20 + m % 11), (b, ts + 9, if m % 5 == 0 { -1 } else { 80 + m % 3 })],
        )
        .unwrap();
    }
    let end = start + 6 * 3_600;

    let compare = |step: i64| {
        let hourly = Span { since: start, step, hourly: true };
        let minutes = Span::minutes(start, step);
        let (got, want) = (db.metrics(id, hourly).unwrap(), db.metrics(id, minutes).unwrap());
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(
                (&g["ts"], &g["net_rx_max"], &g["net_tx_max"], &g["cpu_max"], &g["minutes"]),
                (&w["ts"], &w["net_rx_max"], &w["net_tx_max"], &w["cpu_max"], &w["minutes"])
            );
            assert!((g["cpu"].as_f64().unwrap() - w["cpu"].as_f64().unwrap()).abs() < 1e-9, "{g} {w}");
            let slack = if step == 3_600 { 0 } else { 1 };
            for key in ["mem_used", "disk_used", "net_rx", "net_tx", "swap_used", "tcp", "udp", "procs"] {
                let d = g[key].as_i64().unwrap() - w[key].as_i64().unwrap();
                assert!(d.abs() <= slack, "{key} at {step}s: {g} {w}");
            }
        }
        let ((got, got_loss), (want, want_loss)) =
            (db.ping_records(id, hourly).unwrap(), db.ping_records(id, minutes).unwrap());
        assert_eq!(got_loss, want_loss, "the window's loss is exact");
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(&want) {
            if step == 3_600 {
                assert_eq!(g, w);
            } else {
                assert_eq!((&g["ts"], &g["band"], &g["loss"]), (&w["ts"], &w["band"], &w["loss"]));
                let median = g["latency"].as_i64().unwrap();
                let band = g["band"].as_array().map(|b| (b[0].as_i64().unwrap(), b[1].as_i64().unwrap()));
                assert!(band.is_none_or(|(lo, hi)| (lo..=hi).contains(&median)), "{g}");
            }
        }
    };

    // An hour's median stands for each of its answers: one hour of a single
    // 10 ms answer and one of three at 20 ms are four answers, median 20.
    let mut t = Tally::default();
    t.add(Sample { answered: 1, lost: 0, median: Some(10), lo: Some(10), hi: Some(10) });
    t.add(Sample { answered: 3, lost: 2, median: Some(20), lo: Some(15), hi: Some(40) });
    assert_eq!((t.median(), t.answered(), t.lo, t.hi), (Some(20), 4, Some(10), Some(40)));

    // The last hour waits out the lateness allowance and is read from its
    // minute rows meanwhile.
    assert_eq!(db.roll_up(end + LATE - 1, 365).unwrap(), 5);
    compare(3_600);
    compare(7_200);
    assert_eq!(db.roll_up(end + LATE, 365).unwrap(), 1);
    compare(3_600);
    compare(7_200);

    // Against the fixture itself, as the comparison above would pass two
    // tiers wrong in the same way: every minute counted once, the peak the
    // busiest minute reached.
    let points = db.metrics(id, Span { since: start, step: 7_200, hourly: true }).unwrap();
    let held: usize = kept.iter().map(Vec::len).sum();
    assert_eq!(points.iter().map(|p| p["minutes"].as_i64().unwrap()).sum::<i64>(), held as i64);
    assert_eq!(points.iter().map(|p| p["cpu_max"].as_f64().unwrap()).fold(0.0, f64::max), 9.0 + 10.0);
    // The columns the official themes do not draw are kept as each hour's mean,
    // swap truncated like the other bytes and the counts rounded.
    let stored: Vec<[i64; 4]> = db
        .conn()
        .prepare("SELECT swap_used, tcp, udp, procs FROM metric_hour WHERE node_id=?1 ORDER BY ts")
        .unwrap()
        .query_map([id], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let means: Vec<[i64; 4]> = kept
        .iter()
        .map(|hour| {
            let mean = |i: usize| hour.iter().map(|v| v[i]).sum::<i64>() as f64 / hour.len() as f64;
            [mean(0) as i64, mean(1).round() as i64, mean(2).round() as i64, mean(3).round() as i64]
        })
        .collect();
    assert_eq!(stored, means);
}

/// A backup from before the hourly tier lacks its tables. It must still pass
/// every gate, and come out of the restore with the tables a fresh file has.
#[test]
fn a_backup_from_before_the_hourly_tier_restores() {
    let scratch = Scratch::new();
    let db = Db::open(&scratch.0).unwrap();
    let old = format!("{}.copy", scratch.0);
    release_file(&old);
    db.check_backup(&old).unwrap();
    db.restore_from(&old).unwrap();
    let now = Utc::now().timestamp();
    let hour = (now - 3 * 3_600).div_euclid(3_600) * 3_600;
    db.insert_metric(1, hour, &serde_json::json!({"cpu": 1.0})).unwrap();
    db.roll_up(now, 1).unwrap();
    let span = Span { since: hour, step: 3_600, hourly: true };
    assert_eq!(db.metrics(1, span).unwrap().len(), 1, "the restored file folds and answers");
}

/// The node-side editor sends its ticks and what it opened with; only the
/// difference is written.
#[test]
fn a_node_edit_applies_only_the_probes_it_changed() {
    let db = db();
    let (a, other) = (node(&db, 1), node(&db, 1));
    let probe = |db: &Db, nodes| {
        db.save_ping_task(&PingTask {
            name: "p".into(),
            target: "1.1.1.1:443".into(),
            interval: 60,
            nodes,
            ..Default::default()
        })
        .unwrap()
    };
    let (kept, dropped, added, elsewhere) =
        (probe(&db, vec![a]), probe(&db, vec![a, other]), probe(&db, vec![]), probe(&db, vec![]));
    // Assigned from the probe's side while this node's editor was open.
    db.save_ping_task(&PingTask {
        id: elsewhere,
        name: "p".into(),
        target: "1.1.1.1:443".into(),
        interval: 60,
        nodes: vec![a],
        base: Some(vec![]),
        ..Default::default()
    })
    .unwrap();

    assert!(db.set_node_ping_tasks(a, &[kept, added], &[kept, dropped]).unwrap());
    let ids = |n| db.ping_tasks_for(n).unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect::<Vec<_>>();
    assert_eq!(ids(a), [kept, added, elsewhere]);
    assert_eq!(ids(other), [dropped], "an assignment on another node is unchanged");

    assert!(!db.set_node_ping_tasks(a + 100, &[kept], &[]).unwrap());
    db.delete_ping_task(added).unwrap();
    assert!(db.set_node_ping_tasks(other, &[added], &[]).is_err(), "a deleted probe is refused");
    let many: Vec<i64> = (0..Db::MAX_PROBES_PER_NODE).map(|_| probe(&db, vec![])).collect();
    assert!(db.set_node_ping_tasks(other, &many, &[]).is_err(), "65 probes on one node");
    assert_eq!(ids(other), [dropped], "a refusal writes nothing");

    // Over the limit by another route: editing a different node still saves.
    for task in &many {
        db.conn()
            .execute("INSERT INTO ping_node (task_id, node_id) VALUES (?1,?2)", params![task, other])
            .unwrap();
    }
    assert!(db.set_node_ping_tasks(a, &[kept], &[kept, added]).unwrap());
}
