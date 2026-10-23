use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct Proxy {
    pub id: i64,
    pub node_id: i64,
    pub name: String,
    pub include_node_name: bool,
    pub protocol: String,
    pub address_type: String,
    pub address: String,
    pub port: i64,
    pub enabled: bool,
    pub flow: String,
    pub config: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
    pub sort: i64,
}

#[derive(Debug, Clone)]
pub struct ProxyDraft {
    pub name: String,
    pub include_node_name: bool,
    pub protocol: String,
    pub address_type: String,
    pub address: String,
    pub port: i64,
    pub enabled: bool,
    pub flow: Option<String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyWriteIssue {
    NodeNotFound,
    ProxyNotFound,
    PortConflict,
    NameConflict,
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub uuid: String,
    pub enabled: bool,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub proxy_count: i64,
    pub auto_authorize_new_proxies: bool,
    pub traffic_limit: i64,
    pub device_limit: i64,
    pub traffic_reset_day: u32,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ProxyTrafficCounter {
    pub uplink: i64,
    pub downlink: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyUserTrafficSummary {
    pub user_id: i64,
    pub username: String,
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub last_seen_at: Option<i64>,
    pub reset_at: Option<i64>,
    pub traffic_limit: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyUserTrafficByNode {
    pub user_id: i64,
    pub node_id: i64,
    pub node_name: String,
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub last_seen_at: Option<i64>,
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyNodeTrafficSummary {
    pub node_id: i64,
    pub node_name: String,
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub last_seen_at: Option<i64>,
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyTrafficOverview {
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub total_bytes: i64,
    pub active_users: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyTrafficUserRow {
    pub user_id: i64,
    pub username: String,
    pub node_id: i64,
    pub node_name: String,
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub total_bytes: i64,
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrafficProxySummary {
    pub id: i64,
    pub name: String,
    pub protocol: String,
    pub enabled: bool,
    pub include_node_name: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyTrafficNodeRow {
    pub node_id: i64,
    pub node_name: String,
    pub protocols: Vec<String>,
    pub proxies: Vec<TrafficProxySummary>,
    pub uplink_bytes: i64,
    pub downlink_bytes: i64,
    pub total_bytes: i64,
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxyTrafficPage<T> {
    pub items: Vec<T>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct UserDraft {
    pub username: String,
    /// `None` preserves the current password on update. Creation requires one.
    pub password_hash: Option<String>,
    pub enabled: bool,
    pub expires_at: Option<i64>,
    pub traffic_limit: Option<i64>,
    pub device_limit: Option<i64>,
    pub traffic_reset_day: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPrincipal {
    Admin,
    User(i64),
}

#[derive(Debug, Clone)]
pub struct UserLoginRecord {
    pub id: i64,
    pub password_hash: Option<String>,
    pub enabled: bool,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserPortalProxy {
    pub realm_target_node_id: Option<i64>,
    pub id: i64,
    pub node_id: i64,
    pub node_name: String,
    pub node_group: String,
    pub node_country: String,
    pub name: String,
    pub include_node_name: bool,
    pub protocol: String,
    pub address: String,
    pub address_type: String,
    pub port: i64,
    pub server_name: String,
    pub server_port: i64,
    pub uuid: String,
    pub public_key: String,
    pub short_id: String,
    pub flow: String,
    pub online: bool,
}

#[derive(Debug, Clone)]
pub struct UserSubscriptionProxy {
    pub id: i64,
    pub node_name: String,
    pub node_country: String,
    pub name: String,
    pub include_node_name: bool,
    pub address: String,
    pub port: i64,
    pub server_name: String,
    pub public_key: String,
    pub short_id: String,
    pub flow: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserWriteIssue {
    UserNotFound,
    UsernameConflict,
    AdminUsernameLocked,
    PasswordRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDeleteResult {
    Deleted,
    NotFound,
    Protected,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProxySummary {
    pub id: i64,
    pub node_id: i64,
    pub name: String,
    pub include_node_name: bool,
    pub protocol: String,
    pub address_type: String,
    pub address: String,
    pub port: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorizationRecord {
    pub id: i64,
    pub user_id: i64,
    pub proxy_id: i64,
    pub enabled: bool,
    pub auth: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AuthorizationDraft {
    pub enabled: bool,
    /// Legacy per-authorization settings are optional. New callers omit them;
    /// doing so preserves existing values if a grant is updated in place.
    pub auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationWriteIssue {
    UserNotFound,
    ProxyNotFound,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserProxyAuthorizationItem {
    pub proxy: ProxySummary,
    pub access: AuthorizationSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorizationSummary {
    pub enabled: bool,
    pub auth: serde_json::Value,
}

/// One node's stored configuration and last known facts.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Node {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default = "yes")]
    pub public: bool,
    #[serde(default)]
    pub sort: i64,
    #[serde(default)]
    pub price: f64,
    #[serde(default = "usd")]
    pub currency: String,
    #[serde(default = "monthly")]
    pub billing_cycle: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub remark: String,
    /// Set in the panel for visitors to read, beside `remark`, which they never
    /// see. Empty is none.
    #[serde(default)]
    pub public_remark: String,
    /// Monthly allowance in bytes; 0 means unmetered.
    #[serde(default)]
    pub traffic_limit: i64,
    /// How the allowance is counted: sum, max, up or down.
    #[serde(default = "sum")]
    pub traffic_mode: String,
    #[serde(default = "one")]
    pub traffic_reset_day: u32,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub virt: String,
    #[serde(default)]
    pub cpu_name: String,
    #[serde(default)]
    pub cpu_cores: i64,
    #[serde(default)]
    pub mem_total: i64,
    #[serde(default)]
    pub swap_total: i64,
    #[serde(default)]
    pub disk_total: i64,
    #[serde(default)]
    pub agent_version: String,
    #[serde(default)]
    pub ip: String,
    /// Likewise the address the hub saw, in the family `ip` is not; see the
    /// column.
    #[serde(default)]
    pub ip_other: String,
    /// Reported by the agent from its own interfaces, unlike `ip`, which is
    /// merely the address the agent's connection originated from.
    #[serde(default)]
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
    /// ISO 3166-1 alpha-2, uppercase, or empty when unknown; see
    /// `agent_ws::country_source` for the address it is looked up from. Public:
    /// it appears on the status page beside the node's name.
    #[serde(default)]
    pub country: String,
    /// Set in the panel: two uppercase letters, or empty for the looked-up
    /// `country`. What the status page shows is this when present.
    #[serde(default)]
    pub country_pin: String,
    /// Set in the panel; empty is ungrouped. Public, like the name.
    #[serde(default)]
    pub group: String,
    /// Set in the panel, in canonical form, for what neither agent nor hub can
    /// know: the home line behind a transparent proxy, or which of several public
    /// addresses to show. Each replaces the address shown for its family; empty
    /// is automatic. Panel only, like `ip`.
    #[serde(default)]
    pub ipv4_pin: String,
    #[serde(default)]
    pub ipv6_pin: String,
    /// Unix seconds of the node's last report, written once a minute alongside
    /// the metric row. Zero for a node that has never reported.
    #[serde(default)]
    pub last_seen: i64,
    /// Whether going offline and coming back are announced. See `notify`.
    #[serde(default)]
    pub notify: bool,
    #[serde(default)]
    pub down_since: i64,
    /// What the agent authenticates with. Readable so the panel can display an
    /// install command on demand; it never leaves the admin view.
    #[serde(default)]
    pub token: String,
}

pub(super) fn yes() -> bool {
    true
}

/// Omitted settings stay unchanged. An explicit null clears the expiry date.
#[derive(Deserialize, Default)]
pub struct NodePatch {
    pub name: Option<String>,
    pub public: Option<bool>,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub billing_cycle: Option<String>,
    #[serde(default, deserialize_with = "expiry_patch")]
    pub expires_at: Option<Option<String>>,
    pub remark: Option<String>,
    pub public_remark: Option<String>,
    pub traffic_limit: Option<i64>,
    pub traffic_mode: Option<String>,
    pub traffic_reset_day: Option<u32>,
    pub notify: Option<bool>,
    pub country_pin: Option<String>,
    pub ipv4_pin: Option<String>,
    pub ipv6_pin: Option<String>,
    pub group: Option<String>,
}

pub(super) fn expiry_patch<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
pub struct TrafficPatch {
    pub total_rx: Option<i64>,
    pub total_tx: Option<i64>,
    pub month_rx: Option<i64>,
    pub month_tx: Option<i64>,
}
pub(super) fn usd() -> String {
    "USD".into()
}
pub(super) fn monthly() -> String {
    "monthly".into()
}
pub(super) fn sum() -> String {
    "sum".into()
}
pub(super) fn one() -> u32 {
    1
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
pub struct Traffic {
    pub total_rx: i64,
    pub total_tx: i64,
    pub month_rx: i64,
    pub month_tx: i64,
    pub month_start: String,
    pub day_rx: i64,
    pub day_tx: i64,
}

impl Traffic {
    /// This period's usage as the node's plan meters it. Summing both directions
    /// regardless would hold a plan billed on upload alone against the wrong
    /// figure. The traffic alert and `node_view` both read this, so the
    /// percentage an alert quotes matches the usage the pages show.
    pub fn month_used(&self, traffic_mode: &str) -> i64 {
        match traffic_mode {
            "up" => self.month_tx,
            "down" => self.month_rx,
            "max" => self.month_rx.max(self.month_tx),
            _ => self.month_rx.saturating_add(self.month_tx),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct PingTask {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub interval: i64,
    #[serde(default)]
    pub nodes: Vec<i64>,
    /// Nodes created later are assigned this probe as they are added. Existing
    /// nodes follow `nodes` alone.
    #[serde(default)]
    pub auto_join: bool,
    /// The assignments the editor started from. When given on an update, only
    /// the difference between it and `nodes` is applied, so an assignment made
    /// while the editor was open -- a node joining through `auto_join` -- is
    /// not removed by a list that predates it.
    #[serde(default, skip_serializing)]
    pub base: Option<Vec<i64>>,
}

pub(super) const HOURLY_TABLES: [&str; 2] = ["metric_hour", "ping_hour"];

/// Points SQLite's temporary files -- the copy `VACUUM` rebuilds the database
/// into, and any sort too large for memory -- at the directory holding the
/// database. Every deployment keeps that directory writable and sized for the
/// database (the unit's `ReadWritePaths`, the image's /data), and the backup and
/// restore scratch files already go there.
///
/// SQLite otherwise tries $SQLITE_TMPDIR, $TMPDIR, /var/tmp, /usr/tmp, /tmp and
/// the working directory. The Docker image is built from scratch and has none of
/// them writable. The rebuilt copy stays in a page cache sized like the main
/// database's 8 MiB and needs a file only beyond it, so `VACUUM` would succeed
/// on a database compacting to 6.3 MB and fail on one compacting to 9 MB, with
/// "unable to determine a suitable directory for temporary files". SQLite
/// unlinks each file as it opens it, so none remain there.
///
/// Process-wide and not thread-safe, so it is called once, before any
/// connection is opened. A bare file name keeps SQLite's own search, which ends
/// at the working directory holding it.
pub fn temp_files_beside(database: &str) -> Result<()> {
    let Some(dir) = std::path::Path::new(database).parent().filter(|d| !d.as_os_str().is_empty()) else {
        return Ok(());
    };
    let dir = dir.to_string_lossy().replace('\'', "''");
    Connection::open_in_memory()?.execute_batch(&format!("PRAGMA temp_store_directory = '{dir}'"))?;
    Ok(())
}
