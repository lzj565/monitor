//! ProxyUser 的统一实际访问状态规则。

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyUserAccessState {
    Enabled,
    AdminDisabled,
    Expired,
    TrafficExceeded,
}

impl ProxyUserAccessState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::AdminDisabled => "admin_disabled",
            Self::Expired => "expired",
            Self::TrafficExceeded => "traffic_exceeded",
        }
    }
}

pub fn effective_proxy_access(
    enabled: bool,
    expire_at: Option<i64>,
    traffic_limit_bytes: i64,
    uplink_bytes: i64,
    downlink_bytes: i64,
    now: i64,
) -> ProxyUserAccessState {
    if !enabled {
        ProxyUserAccessState::AdminDisabled
    } else if expire_at.is_some_and(|expires| now >= expires) {
        ProxyUserAccessState::Expired
    } else if traffic_limit_bytes > 0 && uplink_bytes.saturating_add(downlink_bytes) >= traffic_limit_bytes {
        ProxyUserAccessState::TrafficExceeded
    } else {
        ProxyUserAccessState::Enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_state_uses_operator_expiry_and_combined_traffic_in_order() {
        let state = |enabled, expires, limit, up, down, now| {
            effective_proxy_access(enabled, expires, limit, up, down, now)
        };
        assert_eq!(state(false, Some(10), 1, 9, 9, 10), ProxyUserAccessState::AdminDisabled);
        assert_eq!(state(true, Some(10), 0, 0, 0, 10), ProxyUserAccessState::Expired);
        assert_eq!(state(true, Some(11), 10, 4, 6, 10), ProxyUserAccessState::TrafficExceeded);
        assert_eq!(state(true, None, 10, 4, 5, 10), ProxyUserAccessState::Enabled);
        assert_eq!(state(true, None, 10, 4, 7, 10), ProxyUserAccessState::TrafficExceeded);
        assert_eq!(state(true, None, 0, i64::MAX, 1, 10), ProxyUserAccessState::Enabled);
    }
}
