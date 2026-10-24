//! Saved install intent and independently observed component inventory.
use super::*;
use serde_json::{json, Value};
pub(super) const COMPONENT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS node_components(
 node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
 component TEXT NOT NULL CHECK(component IN ('singbox','realm','mieru')),
 desired_installed INTEGER NOT NULL CHECK(desired_installed IN (0,1)),
 installed INTEGER NOT NULL DEFAULT 0 CHECK(installed IN (0,1)),
 managed INTEGER NOT NULL DEFAULT 0 CHECK(managed IN (0,1)),
 running INTEGER NOT NULL DEFAULT 0 CHECK(running IN (0,1)),
 version TEXT NOT NULL DEFAULT '',checked_at INTEGER,last_error TEXT NOT NULL DEFAULT '',
 PRIMARY KEY(node_id,component),CHECK(component<>'singbox' OR desired_installed=1)
);
"#;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallChoices {
    #[serde(default = "required_singbox")]
    pub singbox: bool,
    pub realm: bool,
    pub mieru: bool,
}
fn required_singbox() -> bool {
    true
}
impl Default for InstallChoices {
    fn default() -> Self {
        Self { singbox: true, realm: false, mieru: false }
    }
}
impl InstallChoices {
    pub fn validate(&self) -> Result<()> {
        if !self.singbox {
            refuse!("sing-box 必须安装，不能关闭")
        }
        Ok(())
    }
}
pub(super) fn seed_components(conn: &Connection, id: i64, choices: &InstallChoices) -> Result<()> {
    choices.validate()?;
    for (component, desired) in [("singbox", true), ("realm", choices.realm), ("mieru", choices.mieru)] {
        conn.execute(
            "INSERT INTO node_components(node_id,component,desired_installed) VALUES(?1,?2,?3)",
            params![id, component, desired],
        )?;
    }
    Ok(())
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentObservation {
    pub installed: Option<bool>,
    pub managed: Option<bool>,
    pub running: Option<bool>,
    pub version: Option<String>,
    pub error: Option<String>,
}
impl Db {
    pub fn component_choices(&self, id: i64) -> Result<Option<InstallChoices>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT component,desired_installed FROM node_components WHERE node_id=?1")?;
        let rows = stmt
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(rows.len() == 3, "incomplete component profile");
        let mut choices = InstallChoices::default();
        for (c, on) in rows {
            match c.as_str() {
                "realm" => choices.realm = on,
                "mieru" => choices.mieru = on,
                "singbox" => choices.singbox = on,
                _ => unreachable!(),
            }
        }
        choices.validate()?;
        Ok(Some(choices))
    }
    pub fn component_profile(&self, id: i64) -> Result<Option<Value>> {
        let Some(install) = self.component_choices(id)? else { return Ok(None) };
        let conn = self.conn();
        let mut stmt=conn.prepare("SELECT component,desired_installed,installed,managed,running,version,checked_at,last_error FROM node_components WHERE node_id=?1 ORDER BY CASE component WHEN 'singbox' THEN 0 WHEN 'realm' THEN 1 ELSE 2 END")?;
        let components=stmt.query_map([id],|r|Ok(json!({"component":r.get::<_,String>(0)?,"desired_installed":r.get::<_,bool>(1)?,"installed":r.get::<_,bool>(2)?,"managed":r.get::<_,bool>(3)?,"running":r.get::<_,bool>(4)?,"version":r.get::<_,String>(5)?,"checked_at":r.get::<_,Option<i64>>(6)?,"last_error":r.get::<_,String>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(json!({"install":install,"components":components})))
    }
    pub fn save_component_choices(&self, id: i64, choices: &InstallChoices) -> Result<bool> {
        choices.validate()?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool =
            tx.query_row("SELECT EXISTS(SELECT 1 FROM node WHERE id=?1)", [id], |r| r.get(0))?;
        if !exists {
            return Ok(false);
        }
        for (component, desired) in [("singbox", true), ("realm", choices.realm), ("mieru", choices.mieru)] {
            tx.execute(
                "UPDATE node_components SET desired_installed=?3 WHERE node_id=?1 AND component=?2",
                params![id, component, desired],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }
    pub fn record_components(
        &self,
        id: i64,
        observations: &HashMap<String, ComponentObservation>,
        now: i64,
    ) -> Result<()> {
        anyhow::ensure!(observations.len() <= 3, "too many components");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (component, o) in observations {
            anyhow::ensure!(matches!(component.as_str(), "singbox" | "realm" | "mieru"), "invalid component");
            anyhow::ensure!(
                o.version.as_ref().is_none_or(|v| v.len() <= 256)
                    && o.error.as_ref().is_none_or(|v| v.len() <= 512),
                "oversized component observation"
            );
            anyhow::ensure!(
                o.error.as_ref().is_some_and(|e| !e.is_empty())
                    || (o.installed.is_some()
                        && o.managed.is_some()
                        && o.running.is_some()
                        && o.version.is_some()),
                "incomplete component observation"
            );
            tx.execute("UPDATE node_components SET installed=COALESCE(?3,installed),managed=COALESCE(?4,managed),running=COALESCE(?5,running),version=COALESCE(?6,version),checked_at=?7,last_error=?8 WHERE node_id=?1 AND component=?2",params![id,component,o.installed,o.managed,o.running,o.version,now,o.error.as_deref().unwrap_or("")])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Db, i64) {
        let db = Db::open(":memory:").unwrap();
        let id = db.create_node(&Node { name: "components".into(), ..Default::default() }, "token").unwrap();
        (db, id)
    }
    #[test]
    fn defaults_persistence_and_inventory_are_independent() {
        let (db, id) = fixture();
        assert_eq!(db.component_choices(id).unwrap(), Some(InstallChoices::default()));
        let initial = db.component_profile(id).unwrap().unwrap();
        assert_eq!(initial["components"].as_array().unwrap().len(), 3);
        assert!(initial["components"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["installed"] == false && c["checked_at"].is_null()));
        let wanted = InstallChoices { singbox: true, realm: true, mieru: true };
        assert!(db.save_component_choices(id, &wanted).unwrap());
        let observation=serde_json::from_value(json!({"realm":{"installed":false,"managed":false,"running":false,"version":""},"singbox":{"installed":true,"managed":true,"running":true,"version":"test-v1"}})).unwrap();
        db.record_components(id, &observation, 10).unwrap();
        assert_eq!(db.component_choices(id).unwrap(), Some(wanted.clone()));
        let error = serde_json::from_value(json!({"singbox":{"error":"timed out"}})).unwrap();
        db.record_components(id, &error, 20).unwrap();
        let p = db.component_profile(id).unwrap().unwrap();
        assert_eq!(p["components"][0]["installed"], true);
        assert_eq!(p["components"][0]["last_error"], "timed out");
        assert_eq!(p["components"][0]["checked_at"], 20);
        assert!(db
            .save_component_choices(id, &InstallChoices { singbox: false, realm: false, mieru: false })
            .is_err());
        assert_eq!(db.component_choices(id).unwrap(), Some(wanted));
        assert!(db
            .conn()
            .execute(
                "UPDATE node_components SET desired_installed=0 WHERE node_id=?1 AND component='singbox'",
                [id]
            )
            .is_err());
    }
    #[test]
    fn malformed_inventory_is_atomic_and_cannot_change_intent() {
        let (db, id) = fixture();
        let before = db.component_profile(id).unwrap();
        let invalid=serde_json::from_value(json!({"realm":{"installed":true,"managed":true,"running":true,"version":"1"},"other":{"error":"bad"}})).unwrap();
        assert!(db.record_components(id, &invalid, 42).is_err());
        assert_eq!(db.component_profile(id).unwrap(), before);
        assert!(serde_json::from_value::<HashMap<String, ComponentObservation>>(
            json!({"realm":{"desired_installed":true}})
        )
        .is_err());
        db.conn().execute("DELETE FROM node WHERE id=?1", [id]).unwrap();
        assert!(db.component_profile(id).unwrap().is_none());
    }
    #[test]
    fn v25_migration_seeds_defaults_once_and_preserves_saved_choices() {
        let (db, id) = fixture();
        db.conn().execute_batch("DROP TABLE node_components; PRAGMA user_version=25;").unwrap();
        migrate(&db.conn(), 25).unwrap();
        assert_eq!(db.component_choices(id).unwrap(), Some(InstallChoices::default()));
        let choices = InstallChoices { singbox: true, realm: true, mieru: false };
        db.save_component_choices(id, &choices).unwrap();
        migrate(&db.conn(), 25).unwrap();
        assert_eq!(db.component_choices(id).unwrap(), Some(choices));
    }
    #[test]
    fn registration_stores_choices_atomically() {
        let (db, _) = fixture();
        let choices = InstallChoices { singbox: true, realm: true, mieru: true };
        let id = db
            .create_node_with_components(
                &Node { name: "registered".into(), ..Default::default() },
                "registration-token",
                &choices,
            )
            .unwrap();
        assert_eq!(db.component_choices(id).unwrap(), Some(choices));
        assert!(db
            .create_node_with_components(
                &Node::default(),
                "invalid-token",
                &InstallChoices { singbox: false, realm: false, mieru: false }
            )
            .is_err());
        assert_eq!(db.node_by_token("invalid-token").unwrap(), None);
    }
}
