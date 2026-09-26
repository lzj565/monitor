//! Hub 本地时区下的日期边界转换，代理用户周期与到期统一使用此处。

use anyhow::{bail, Result};
use chrono::{
    DateTime, Datelike, Duration, Local, LocalResult, Months, NaiveDate, NaiveDateTime, TimeZone, Utc,
};

fn local_timestamp(value: NaiveDateTime) -> Result<i64> {
    match Local.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(value.timestamp()),
        LocalResult::Ambiguous(first, second) => Ok(first.min(second).timestamp()),
        LocalResult::None => {
            // 少数时区会在午夜切换夏令时；边界取当天第一个有效本地时刻。
            for second in 1..=86_400 {
                let Some(candidate) = value.checked_add_signed(Duration::seconds(second)) else { break };
                match Local.from_local_datetime(&candidate) {
                    LocalResult::Single(value) => return Ok(value.timestamp()),
                    LocalResult::Ambiguous(first, second) => return Ok(first.min(second).timestamp()),
                    LocalResult::None => {}
                }
            }
            bail!("Hub 时区中的本地日期无有效时间")
        }
    }
}

pub fn date_start_timestamp(date: NaiveDate) -> Result<i64> {
    let start = date.and_hms_opt(0, 0, 0).ok_or_else(|| anyhow::anyhow!("日期无效"))?;
    local_timestamp(start)
}

/// 日期选择器选中的日期全天有效，在次日 Hub 本地零点失效。
pub fn expiry_date_timestamp(date: NaiveDate) -> Result<i64> {
    let next_day =
        date.checked_add_days(chrono::Days::new(1)).ok_or_else(|| anyhow::anyhow!("日期超出范围"))?;
    date_start_timestamp(next_day)
}

/// 将到期边界还原为用户选择的日期，使用日历日回退以适配夏令时。
pub fn expiry_timestamp_date(timestamp: i64) -> Option<String> {
    let boundary = DateTime::<Utc>::from_timestamp(timestamp, 0)?.with_timezone(&Local).date_naive();
    Some(boundary.pred_opt()?.format("%Y-%m-%d").to_string())
}

pub fn current_period_start(timestamp: i64, reset_day: u8) -> Result<i64> {
    if !(1..=28).contains(&reset_day) {
        bail!("流量重置日必须在 1 到 28 之间");
    }
    let now = DateTime::<Utc>::from_timestamp(timestamp, 0)
        .ok_or_else(|| anyhow::anyhow!("时间戳无效"))?
        .with_timezone(&Local);
    let today = now.date_naive();
    let this_month = today.with_day(u32::from(reset_day)).ok_or_else(|| anyhow::anyhow!("重置日期无效"))?;
    let boundary = if today >= this_month {
        this_month
    } else {
        this_month.checked_sub_months(Months::new(1)).ok_or_else(|| anyhow::anyhow!("重置日期超出范围"))?
    };
    date_start_timestamp(boundary)
}

pub fn now_timestamp() -> i64 {
    Local::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date parses")
    }

    #[test]
    fn selected_expiry_date_is_valid_until_the_next_local_midnight() {
        let selected = date("2026-10-25");
        let expires = expiry_date_timestamp(selected).expect("date converts");
        assert_eq!(expiry_timestamp_date(expires).as_deref(), Some("2026-10-25"));
        assert!(expires > date_start_timestamp(selected).expect("date start"));
    }

    #[test]
    fn reset_period_uses_the_latest_monthly_boundary() {
        let before = date_start_timestamp(date("2026-10-04")).expect("date start") + 12 * 60 * 60;
        let after = date_start_timestamp(date("2026-10-05")).expect("date start") + 60;
        assert_eq!(
            current_period_start(before, 5).expect("period"),
            date_start_timestamp(date("2026-09-05")).expect("date start")
        );
        assert_eq!(
            current_period_start(after, 5).expect("period"),
            date_start_timestamp(date("2026-10-05")).expect("date start")
        );
        assert!(current_period_start(after, 29).is_err());
    }
}
