//! In-memory command tracking for the control channel to connected agents.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

pub const PENDING_TIMEOUT: Duration = Duration::from_secs(35);
pub const RESULT_TTL: Duration = Duration::from_secs(5 * 60);
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5);
const MAX_COMMANDS: usize = 1024;

const ACTIONS: [&str; 5] = [
    "singbox.status",
    "singbox.config.get",
    "singbox.config.check",
    "singbox.config.apply",
    "singbox.restart",
];

pub fn supports(method: &str) -> bool {
    ACTIONS.contains(&method)
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandView {
    pub command_id: String,
    pub method: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

#[derive(Debug)]
enum State {
    Pending { started: Instant },
    Succeeded { result: Value, finished: Instant },
    Failed { error: CommandError, finished: Instant },
}

#[derive(Debug)]
struct Entry {
    node_id: i64,
    session: u64,
    method: String,
    state: State,
}

#[derive(Default)]
pub struct Registry {
    entries: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    Full,
}

impl Registry {
    /// Reserves a result slot before the command is put on the agent's bounded
    /// outbound queue, so even a very fast response has somewhere to complete.
    pub fn start(&self, node_id: i64, session: u64, method: &str) -> Result<String, StartError> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        sweep(&mut entries, now);
        if entries.len() >= MAX_COMMANDS {
            return Err(StartError::Full);
        }
        let id = loop {
            let candidate = crate::auth::random_token();
            if !entries.contains_key(&candidate) {
                break candidate;
            }
        };
        entries.insert(
            id.clone(),
            Entry { node_id, session, method: method.to_owned(), state: State::Pending { started: now } },
        );
        Ok(id)
    }

    /// Removes a reservation when the outbound queue refuses it. A completed
    /// or disconnected command is deliberately left alone.
    pub fn cancel(&self, id: &str, node_id: i64, session: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.get(id).is_some_and(|e| {
            e.node_id == node_id && e.session == session && matches!(e.state, State::Pending { .. })
        }) {
            entries.remove(id);
        }
    }

    /// Completes a command only for the node and socket session that sent it.
    pub fn complete(
        &self,
        id: &str,
        node_id: i64,
        session: u64,
        result: Result<Value, CommandError>,
    ) -> bool {
        self.complete_at(id, node_id, session, result, Instant::now())
    }

    fn complete_at(
        &self,
        id: &str,
        node_id: i64,
        session: u64,
        result: Result<Value, CommandError>,
        now: Instant,
    ) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        sweep(&mut entries, now);
        let Some(entry) = entries.get_mut(id) else { return false };
        if entry.node_id != node_id
            || entry.session != session
            || !matches!(entry.state, State::Pending { .. })
        {
            return false;
        }
        entry.state = match result {
            Ok(result) => State::Succeeded { result, finished: now },
            Err(error) => State::Failed { error, finished: now },
        };
        true
    }

    /// Fails every command belonging to a socket that has ended or been
    /// replaced. A later response from that socket can no longer change it.
    pub fn disconnect(&self, node_id: i64, session: u64) {
        self.disconnect_at(node_id, session, Instant::now());
    }

    fn disconnect_at(&self, node_id: i64, session: u64, now: Instant) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        sweep(&mut entries, now);
        for entry in entries.values_mut().filter(|e| e.node_id == node_id && e.session == session) {
            if matches!(entry.state, State::Pending { .. }) {
                entry.state = State::Failed {
                    error: CommandError {
                        code: "AGENT_DISCONNECTED".into(),
                        message: "agent connection ended before the command completed".into(),
                    },
                    finished: now,
                };
            }
        }
    }

    pub fn get(&self, id: &str, node_id: i64) -> Option<CommandView> {
        self.get_at(id, node_id, Instant::now())
    }

    fn get_at(&self, id: &str, node_id: i64, now: Instant) -> Option<CommandView> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        sweep(&mut entries, now);
        let entry = entries.get(id).filter(|e| e.node_id == node_id)?;
        let (status, result, error) = match &entry.state {
            State::Pending { .. } => ("pending", None, None),
            State::Succeeded { result, .. } => ("succeeded", Some(result.clone()), None),
            State::Failed { error, .. } => ("failed", None, Some(error.clone())),
        };
        Some(CommandView { command_id: id.to_owned(), method: entry.method.clone(), status, result, error })
    }

    pub fn sweep(&self) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        sweep(&mut entries, Instant::now());
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

fn sweep(entries: &mut HashMap<String, Entry>, now: Instant) {
    entries.retain(|_, entry| match &entry.state {
        State::Pending { started } if now.saturating_duration_since(*started) >= PENDING_TIMEOUT => {
            entry.state = State::Failed {
                error: CommandError {
                    code: "COMMAND_TIMEOUT".into(),
                    message: "agent did not return a result before the command timed out".into(),
                },
                finished: now,
            };
            true
        }
        State::Pending { .. } => true,
        State::Succeeded { finished, .. } | State::Failed { finished, .. } => {
            now.saturating_duration_since(*finished) < RESULT_TTL
        }
    });
}

pub async fn cleanup_loop(app: crate::Shared) {
    let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        app.commands.sweep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_only_the_five_supported_actions() {
        for action in ACTIONS {
            assert!(supports(action));
        }
        for action in ["singbox.start", "singbox.stop", "singbox.reload", "singbox.other"] {
            assert!(!supports(action));
        }
    }

    #[test]
    fn completion_is_bound_to_node_and_session_and_preserves_result() {
        let registry = Registry::default();
        let id = registry.start(7, 11, "singbox.status").unwrap();
        assert!(!registry.complete(&id, 8, 11, Ok(serde_json::json!({"running": true}))));
        assert!(!registry.complete(&id, 7, 12, Ok(serde_json::json!({"running": true}))));
        assert!(registry.complete(&id, 7, 11, Ok(serde_json::json!({"running": true}))));
        let view = registry.get(&id, 7).unwrap();
        assert_eq!(view.status, "succeeded");
        assert_eq!(view.result.unwrap()["running"], true);
        assert!(registry.get(&id, 8).is_none());
    }

    #[test]
    fn agent_error_and_disconnect_are_terminal() {
        let registry = Registry::default();
        let id = registry.start(7, 11, "singbox.restart").unwrap();
        let error = CommandError { code: "SINGBOX_ERROR".into(), message: "restart failed".into() };
        assert!(registry.complete(&id, 7, 11, Err(error.clone())));
        assert_eq!(registry.get(&id, 7).unwrap().error.unwrap().code, "SINGBOX_ERROR");

        let disconnected = registry.start(7, 11, "singbox.config.get").unwrap();
        registry.disconnect(7, 11);
        let view = registry.get(&disconnected, 7).unwrap();
        assert_eq!(view.status, "failed");
        assert_eq!(view.error.unwrap().code, "AGENT_DISCONNECTED");
        assert!(!registry.complete(&disconnected, 7, 11, Ok(Value::Null)));
    }

    #[test]
    fn timeout_and_terminal_retention_are_swept() {
        let registry = Registry::default();
        let now = Instant::now();
        let id = {
            let mut entries = registry.entries.lock().unwrap();
            entries.insert(
                "pending".into(),
                Entry {
                    node_id: 1,
                    session: 2,
                    method: "singbox.status".into(),
                    state: State::Pending { started: now },
                },
            );
            "pending".to_owned()
        };
        assert_eq!(
            registry.get_at(&id, 1, now + PENDING_TIMEOUT).unwrap().error.unwrap().code,
            "COMMAND_TIMEOUT"
        );
        assert!(registry.get_at(&id, 1, now + PENDING_TIMEOUT + RESULT_TTL).is_none());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_capacity_is_bounded_and_expired_records_make_room() {
        let registry = Registry::default();
        for node in 0..MAX_COMMANDS as i64 {
            registry.start(node, 1, "singbox.status").unwrap();
        }
        assert_eq!(registry.start(2000, 1, "singbox.status"), Err(StartError::Full));
        let expired_id = {
            let mut entries = registry.entries.lock().unwrap();
            let id = entries.keys().next().unwrap().clone();
            let entry = entries.get_mut(&id).unwrap();
            entry.state = State::Failed {
                error: CommandError { code: "X".into(), message: "x".into() },
                finished: Instant::now() - RESULT_TTL,
            };
            id
        };
        assert!(registry.get(&expired_id, 0).is_none());
        assert!(registry.start(2000, 1, "singbox.status").is_ok());
    }
}
