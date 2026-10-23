use super::*;

impl Db {
    // ---- sessions ----

    pub fn create_session(&self, token_hash: &str, expires_at: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO session (token_hash, expires_at,principal_type,user_id) VALUES (?1, ?2,'admin',NULL)",
            params![token_hash, expires_at],
        )?;
        Ok(())
    }

    pub fn create_user_session(&self, token_hash: &str, expires_at: i64, user_id: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO session (token_hash,expires_at,principal_type,user_id)
             VALUES (?1,?2,'user',?3)",
            params![token_hash, expires_at, user_id],
        )?;
        Ok(())
    }

    pub fn session_principal(&self, token_hash: &str) -> Option<SessionPrincipal> {
        let session = {
            let conn = self.conn();
            conn.query_row(
                "SELECT principal_type,user_id FROM session WHERE token_hash=?1 AND expires_at>?2",
                params![token_hash, Utc::now().timestamp()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .ok()
            .flatten()
        }?;
        match (session.0.as_str(), session.1) {
            ("admin", _) => Some(SessionPrincipal::Admin),
            ("user", Some(id)) if self.user_session_user_id(token_hash) == Some(id) => {
                Some(SessionPrincipal::User(id))
            }
            _ => None,
        }
    }

    pub fn user_session_user_id(&self, token_hash: &str) -> Option<i64> {
        let now = Utc::now().timestamp();
        self.conn()
            .query_row(
                "SELECT s.user_id FROM session s JOIN users u ON u.id=s.user_id
             WHERE s.token_hash=?1 AND s.expires_at>?2 AND s.principal_type='user'
               AND u.enabled=1 AND (u.expires_at IS NULL OR u.expires_at>?2)",
                params![token_hash, now],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn admin_session_valid(&self, token_hash: &str) -> bool {
        matches!(self.session_principal(token_hash), Some(SessionPrincipal::Admin))
    }

    pub fn session_valid(&self, token_hash: &str) -> bool {
        self.conn()
            .query_row(
                "SELECT 1 FROM session WHERE token_hash=?1 AND expires_at > ?2",
                params![token_hash, Utc::now().timestamp()],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Live sessions, newest first. Expired rows are filtered here rather than
    /// left to `expire_sessions`, which sweeps only once an hour.
    pub fn sessions(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT token_hash, expires_at FROM session WHERE expires_at > ?1 ORDER BY expires_at DESC",
        )?;
        let rows = stmt
            .query_map([Utc::now().timestamp()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn drop_session(&self, token_hash: &str) -> Result<()> {
        self.conn().execute("DELETE FROM session WHERE token_hash=?1", [token_hash])?;
        Ok(())
    }

    /// Replaces the admin password hash and signs every session out, both or
    /// neither: a reset that stored the hash and then failed would report failure
    /// while the old password no longer works.
    pub fn replace_password(&self, hash: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO setting (key, value) VALUES ('admin_password_hash', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [hash],
        )?;
        tx.execute("DELETE FROM session", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Invalidates every login. Used after a restore, which would otherwise
    /// revive every session the backup holds.
    pub fn drop_all_sessions(&self) -> Result<()> {
        self.conn().execute("DELETE FROM session", [])?;
        Ok(())
    }

    pub fn expire_sessions(&self) -> Result<()> {
        self.conn().execute("DELETE FROM session WHERE expires_at <= ?1", [Utc::now().timestamp()])?;
        Ok(())
    }
}
