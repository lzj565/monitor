//! 通过既有 Agent 命令桥采集 sing-box counter，并在 Hub 内累计用户流量。

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::task::JoinSet;
use tracing::{debug, warn};

use crate::db::ProxyUserTrafficSample;
use crate::{command, hub_time, proxy_deploy, Shared};

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const ACCESS_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);
const MAX_PARALLEL_SERVERS: usize = 8;
const MAX_USERS_PER_REPORT: usize = 10_000;

fn parse_report(value: &Value) -> Result<(i64, Vec<ProxyUserTrafficSample>)> {
    let uptime = value
        .get("uptime_secs")
        .and_then(Value::as_u64)
        .and_then(|value| i64::try_from(value).ok())
        .context("Agent 流量报告缺少有效 uptime_secs")?;
    let users = value.get("users").and_then(Value::as_array).context("Agent 流量报告缺少 users 数组")?;
    if users.len() > MAX_USERS_PER_REPORT {
        bail!("Agent 流量报告用户数量超过限制");
    }
    let mut seen = HashSet::with_capacity(users.len());
    let mut samples = Vec::with_capacity(users.len());
    for user in users {
        let user_id = user
            .get("user_id")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .filter(|value| *value > 0)
            .context("Agent 流量报告包含无效 user_id")?;
        let user_key =
            user.get("user_key").and_then(Value::as_str).context("Agent 流量报告包含无效 user_key")?;
        if user_key != format!("monitor-user-{user_id}") || !seen.insert(user_id) {
            bail!("Agent 流量报告的用户身份不匹配或重复");
        }
        let uplink_bytes = user
            .get("uplink_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .context("Agent 流量报告包含无效 uplink counter")?;
        let downlink_bytes = user
            .get("downlink_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| i64::try_from(value).ok())
            .context("Agent 流量报告包含无效 downlink counter")?;
        samples.push(ProxyUserTrafficSample {
            user_id,
            user_key: user_key.to_owned(),
            uplink_bytes,
            downlink_bytes,
        });
    }
    Ok((uptime, samples))
}

async fn poll_server(app: &Shared, server_id: i64) -> Result<Vec<i64>> {
    // 代次快照先于 RPC 获取；重置期间返回的旧报告会因代次变化被整份丢弃。
    let expected_generations = app.db.proxy_user_generations_for_node(server_id)?;
    let report = command::singbox_stats_users(app, server_id)
        .await
        .map_err(|error| anyhow::anyhow!("Agent 流量命令失败: {error:?}"))?;
    let (uptime_secs, mut samples) = parse_report(&report)?;
    samples.retain(|sample| {
        if expected_generations.contains_key(&sample.user_id) {
            true
        } else {
            debug!(server_id, user_id = sample.user_id, "忽略未知或未授权的代理用户流量");
            false
        }
    });
    app.db.apply_proxy_user_traffic_samples(
        server_id,
        uptime_secs,
        &expected_generations,
        &samples,
        hub_time::now_timestamp(),
    )
}

async fn deploy_changed_servers(app: &Shared, server_ids: &[i64]) {
    let mut ids = server_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return;
    }
    let failures = proxy_deploy::deploy_proxy_user_servers(app, &ids).await;
    for failure in failures {
        warn!(
            server_id = failure.server_id,
            server = %failure.server_name,
            error = %failure.error,
            "代理用户访问状态变化后的配置同步失败"
        );
    }
}

async fn poll_round(app: &Shared) {
    let server_ids = {
        let agents = app.agents.read().unwrap_or_else(|error| error.into_inner());
        agents
            .iter()
            .filter_map(|(server_id, agent)| {
                agent.capabilities.contains(command::SINGBOX_STATS_USERS_METHOD).then_some(*server_id)
            })
            .collect::<Vec<_>>()
    };
    let mut affected = Vec::new();
    for group in server_ids.chunks(MAX_PARALLEL_SERVERS) {
        let mut jobs = JoinSet::new();
        for server_id in group.iter().copied() {
            let app = app.clone();
            jobs.spawn(async move { (server_id, poll_server(&app, server_id).await) });
        }
        while let Some(result) = jobs.join_next().await {
            match result {
                Ok((_server_id, Ok(changed))) => affected.extend(changed),
                Ok((server_id, Err(error))) => debug!(server_id, "sing-box 用户流量采集失败: {error:#}"),
                Err(error) => warn!("sing-box 用户流量采集任务异常退出: {error}"),
            }
        }
    }
    deploy_changed_servers(app, &affected).await;
}

/// 每台在线且声明支持该 capability 的服务器每轮只发一个 Stats RPC。
pub async fn run_poller(app: Shared) {
    loop {
        poll_round(&app).await;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn maintain_access(app: &Shared) -> Result<()> {
    let now = hub_time::now_timestamp();
    let users = app.db.proxy_users()?;
    let mut affected = Vec::new();
    for user in users {
        let period_start = hub_time::current_period_start(now, user.traffic_reset_day)?;
        if user.period_started_at < period_start {
            if let Some(server_ids) =
                app.db.reset_proxy_user_traffic_for_period(user.id, period_start, now)?
            {
                affected.extend(server_ids);
            }
        }
    }
    affected.extend(app.db.refresh_proxy_user_access_states(now)?);
    deploy_changed_servers(app, &affected).await;
    Ok(())
}

/// 启动时立即检查一次，之后每分钟处理月度周期和到期状态。
pub async fn run_access_maintenance(app: Shared) {
    loop {
        if let Err(error) = maintain_access(&app).await {
            warn!("代理用户周期与到期检查失败: {error:#}");
        }
        tokio::time::sleep(ACCESS_MAINTENANCE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_agent_stats_and_rejects_identity_mismatch_or_duplicate() {
        let report = serde_json::json!({
            "uptime_secs": 42,
            "users": [{
                "user_id": 7,
                "user_key": "monitor-user-7",
                "uplink_bytes": 12,
                "downlink_bytes": 30
            }]
        });
        let (uptime, samples) = parse_report(&report).expect("valid stats response");
        assert_eq!(uptime, 42);
        assert_eq!(samples[0].user_id, 7);
        assert_eq!(samples[0].uplink_bytes, 12);
        assert_eq!(samples[0].downlink_bytes, 30);

        let mismatched = serde_json::json!({
            "uptime_secs": 42,
            "users": [{"user_id": 7, "user_key": "monitor-user-8", "uplink_bytes": 0, "downlink_bytes": 0}]
        });
        assert!(parse_report(&mismatched).is_err());
        let duplicate = serde_json::json!({
            "uptime_secs": 42,
            "users": [
                {"user_id": 7, "user_key": "monitor-user-7", "uplink_bytes": 0, "downlink_bytes": 0},
                {"user_id": 7, "user_key": "monitor-user-7", "uplink_bytes": 1, "downlink_bytes": 1}
            ]
        });
        assert!(parse_report(&duplicate).is_err());
    }
}
