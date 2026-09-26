//! 客户端订阅使用的分流规则；这里不参与服务器 sing-box 配置下发。

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const RULE_CONFIG_VERSION: u32 = 1;
pub const MAX_CUSTOM_RULE_CONFIG_BYTES: usize = 48 * 1024;

const GEO_SITE_CN_TAG: &str = "geosite-geolocation-cn";
const GEO_SITE_CN_URL: &str =
    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-geolocation-cn.srs";
const GEO_IP_CN_TAG: &str = "geoip-cn";
const GEO_IP_CN_URL: &str = "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-cn.srs";

const SMART_CLASH_RULES: &str = r#"# --- 局域网与私有网络直连 ---
- GEOIP,LAN,DIRECT
- DOMAIN-SUFFIX,local,DIRECT
- DOMAIN-SUFFIX,localhost,DIRECT
- IP-CIDR,127.0.0.0/8,DIRECT
- IP-CIDR,172.16.0.0/12,DIRECT
- IP-CIDR,192.168.0.0/16,DIRECT
- IP-CIDR,10.0.0.0/8,DIRECT
- IP-CIDR,100.64.0.0/10,DIRECT
# --- 常用 AI 智能助手走代理 ---
- DOMAIN-SUFFIX,openai.com,节点选择
- DOMAIN-SUFFIX,chatgpt.com,节点选择
- DOMAIN-SUFFIX,anthropic.com,节点选择
- DOMAIN-SUFFIX,claude.ai,节点选择
- DOMAIN-SUFFIX,oaistatic.com,节点选择
- DOMAIN-SUFFIX,oaiusercontent.com,节点选择
- DOMAIN-KEYWORD,gemini,节点选择
- DOMAIN-KEYWORD,openai,节点选择
# --- 常用海外社交与影音走代理 ---
- DOMAIN-SUFFIX,google.com,节点选择
- DOMAIN-SUFFIX,youtube.com,节点选择
- DOMAIN-SUFFIX,github.com,节点选择
- DOMAIN-SUFFIX,githubusercontent.com,节点选择
- DOMAIN-SUFFIX,twitter.com,节点选择
- DOMAIN-SUFFIX,x.com,节点选择
- DOMAIN-SUFFIX,telegram.org,节点选择
- DOMAIN-SUFFIX,t.me,节点选择
- DOMAIN-SUFFIX,netflix.com,节点选择
- DOMAIN-SUFFIX,spotify.com,节点选择
# --- 国内常见服务与域名直连 ---
- DOMAIN-SUFFIX,cn,DIRECT
- DOMAIN-SUFFIX,baidu.com,DIRECT
- DOMAIN-SUFFIX,qq.com,DIRECT
- DOMAIN-SUFFIX,weixin.com,DIRECT
- DOMAIN-SUFFIX,alipay.com,DIRECT
- DOMAIN-SUFFIX,taobao.com,DIRECT
- DOMAIN-SUFFIX,jd.com,DIRECT
- DOMAIN-SUFFIX,bilibili.com,DIRECT
- DOMAIN-SUFFIX,163.com,DIRECT
- DOMAIN-SUFFIX,zhihu.com,DIRECT
- GEOIP,CN,DIRECT
# --- 最终兜底规则 ---
- MATCH,节点选择"#;

const LITE_CLASH_RULES: &str = r#"# --- 局域网与私有网络直连 ---
- GEOIP,LAN,DIRECT
- DOMAIN-SUFFIX,local,DIRECT
- DOMAIN-SUFFIX,localhost,DIRECT
- IP-CIDR,127.0.0.0/8,DIRECT
- IP-CIDR,172.16.0.0/12,DIRECT
- IP-CIDR,192.168.0.0/16,DIRECT
- IP-CIDR,10.0.0.0/8,DIRECT
- IP-CIDR,100.64.0.0/10,DIRECT
# --- 国内流量直连 ---
- DOMAIN-SUFFIX,cn,DIRECT
- GEOIP,CN,DIRECT
# --- 最终兜底规则 ---
- MATCH,节点选择"#;

const GLOBAL_CLASH_RULES: &str = r#"# --- 局域网与私有网络直连 ---
- GEOIP,LAN,DIRECT
- DOMAIN-SUFFIX,local,DIRECT
- DOMAIN-SUFFIX,localhost,DIRECT
- IP-CIDR,127.0.0.0/8,DIRECT
- IP-CIDR,172.16.0.0/12,DIRECT
- IP-CIDR,192.168.0.0/16,DIRECT
- IP-CIDR,10.0.0.0/8,DIRECT
- IP-CIDR,100.64.0.0/10,DIRECT
# --- 其余流量走代理 ---
- MATCH,节点选择"#;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    #[default]
    Smart,
    Lite,
    Global,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustomRuleConfig {
    /// Mihomo 原生 rules 列表文本；每条规则可带或不带 YAML 的 `- ` 前缀。
    pub clash_rules: String,
    /// sing-box 的完整 route 对象，不包含外层配置中的 `route` 键。
    pub singbox_route: Value,
}

impl Default for CustomRuleConfig {
    fn default() -> Self {
        Self {
            clash_rules: String::new(),
            singbox_route: json!({"rules": [], "rule_set": [], "final": "proxy"}),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuleConfig {
    pub version: u32,
    pub mode: RuleMode,
    pub custom: CustomRuleConfig,
    pub updated_at: i64,
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            version: RULE_CONFIG_VERSION,
            mode: RuleMode::Smart,
            custom: CustomRuleConfig::default(),
            updated_at: 0,
        }
    }
}

impl RuleConfig {
    /// 校验可持久化的配置；预设模式不启用的自定义草稿不会阻止保存。
    pub fn validate(&self) -> Result<()> {
        if self.version != RULE_CONFIG_VERSION {
            bail!("不支持的规则配置版本 {}", self.version);
        }
        if self.mode == RuleMode::Custom {
            let encoded = serde_json::to_vec(&self.custom).context("序列化自定义规则失败")?;
            if encoded.len() > MAX_CUSTOM_RULE_CONFIG_BYTES {
                bail!("自定义规则配置不能超过 {} KiB", MAX_CUSTOM_RULE_CONFIG_BYTES / 1024);
            }
            validate_clash_rules(&self.custom.clash_rules)?;
            validate_singbox_route(&self.custom.singbox_route)?;
        }
        Ok(())
    }
}

/// 按模式生成 Mihomo rules 列表；结果可直接放在 YAML 的 `rules:` 下。
pub fn render_clash_rules(config: &RuleConfig) -> Result<String> {
    let rules = match config.mode {
        RuleMode::Smart => SMART_CLASH_RULES,
        RuleMode::Lite => LITE_CLASH_RULES,
        RuleMode::Global => GLOBAL_CLASH_RULES,
        RuleMode::Custom => return normalize_clash_rules(&config.custom.clash_rules),
    };
    normalize_clash_rules(rules)
}

/// 按模式生成 sing-box route 对象，不转换 Mihomo 规则语法。
pub fn render_singbox_route(config: &RuleConfig) -> Result<Value> {
    if config.mode == RuleMode::Custom {
        validate_singbox_route(&config.custom.singbox_route)?;
        return Ok(config.custom.singbox_route.clone());
    }

    let mut rules = vec![json!({
        "ip_is_private": true,
        "action": "route",
        "outbound": "direct"
    })];
    // GEOIP,LAN 还包括共享地址段；ip_is_private 单独使用时不覆盖它。
    rules.push(json!({
        "ip_cidr": ["127.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "10.0.0.0/8", "100.64.0.0/10"],
        "action": "route",
        "outbound": "direct"
    }));
    rules.push(json!({
        "domain": ["local", "localhost"],
        "action": "route",
        "outbound": "direct"
    }));
    rules.push(json!({
        "domain_suffix": [".local", ".localhost"],
        "action": "route",
        "outbound": "direct"
    }));

    let mut rule_sets = Vec::new();
    match config.mode {
        RuleMode::Smart => {
            // 这些例外需排在 CN 规则前，避免国内 IP 分类覆盖指定代理服务。
            rules.push(json!({
                "domain": [
                    "openai.com", "chatgpt.com", "anthropic.com", "claude.ai",
                    "oaistatic.com", "oaiusercontent.com"
                ],
                "action": "route",
                "outbound": "proxy"
            }));
            rules.push(json!({
                "domain_suffix": [
                    ".openai.com", ".chatgpt.com", ".anthropic.com", ".claude.ai",
                    ".oaistatic.com", ".oaiusercontent.com"
                ],
                "action": "route",
                "outbound": "proxy"
            }));
            rules.push(json!({
                "domain_keyword": ["gemini", "openai"],
                "action": "route",
                "outbound": "proxy"
            }));
            rules.push(json!({
                "domain": [
                    "google.com", "youtube.com", "github.com", "githubusercontent.com",
                    "twitter.com", "x.com", "telegram.org", "t.me", "netflix.com", "spotify.com"
                ],
                "action": "route",
                "outbound": "proxy"
            }));
            rules.push(json!({
                "domain_suffix": [
                    ".google.com", ".youtube.com", ".github.com", ".githubusercontent.com",
                    ".twitter.com", ".x.com", ".telegram.org", ".t.me", ".netflix.com", ".spotify.com"
                ],
                "action": "route",
                "outbound": "proxy"
            }));
            rules.push(json!({
                "domain": [
                    "baidu.com", "qq.com", "weixin.com", "alipay.com", "taobao.com",
                    "jd.com", "bilibili.com", "163.com", "zhihu.com"
                ],
                "action": "route",
                "outbound": "direct"
            }));
            rules.push(json!({
                "domain_suffix": [
                    ".cn", ".baidu.com", ".qq.com", ".weixin.com", ".alipay.com",
                    ".taobao.com", ".jd.com", ".bilibili.com", ".163.com", ".zhihu.com"
                ],
                "action": "route",
                "outbound": "direct"
            }));
            rule_sets.push(remote_rule_set(GEO_SITE_CN_TAG, GEO_SITE_CN_URL));
            rules.push(json!({
                "rule_set": [GEO_SITE_CN_TAG],
                "action": "route",
                "outbound": "direct"
            }));
            rule_sets.push(remote_rule_set(GEO_IP_CN_TAG, GEO_IP_CN_URL));
            rules.push(json!({
                "rule_set": [GEO_IP_CN_TAG],
                "action": "route",
                "outbound": "direct"
            }));
        }
        RuleMode::Lite => {
            rules.push(json!({
                "domain_suffix": [".cn"],
                "action": "route",
                "outbound": "direct"
            }));
            rule_sets.push(remote_rule_set(GEO_SITE_CN_TAG, GEO_SITE_CN_URL));
            rules.push(json!({
                "rule_set": [GEO_SITE_CN_TAG],
                "action": "route",
                "outbound": "direct"
            }));
            rule_sets.push(remote_rule_set(GEO_IP_CN_TAG, GEO_IP_CN_URL));
            rules.push(json!({
                "rule_set": [GEO_IP_CN_TAG],
                "action": "route",
                "outbound": "direct"
            }));
        }
        RuleMode::Global => {}
        RuleMode::Custom => bail!("自定义规则模式不应进入内置规则生成器"),
    }

    Ok(json!({"rules": rules, "rule_set": rule_sets, "final": "proxy"}))
}

pub fn validate_clash_rules(rules: &str) -> Result<()> {
    normalize_clash_rules(rules).map(|_| ())
}

pub fn validate_singbox_route(route: &Value) -> Result<()> {
    let encoded = serde_json::to_vec(route).context("序列化 sing-box route 失败")?;
    if encoded.len() > MAX_CUSTOM_RULE_CONFIG_BYTES {
        bail!("sing-box route 不能超过 {} KiB", MAX_CUSTOM_RULE_CONFIG_BYTES / 1024);
    }
    let Some(object) = route.as_object() else {
        bail!("sing-box route 必须是 JSON 对象");
    };
    reject_removed_route_fields(route, "route")?;

    if !object.get("final").and_then(Value::as_str).is_some_and(|outbound| !outbound.trim().is_empty()) {
        bail!("sing-box route.final 必须是非空字符串");
    }
    let mut referenced_rule_sets = Vec::new();
    if let Some(rules) = object.get("rules") {
        let Some(rules) = rules.as_array() else {
            bail!("sing-box route.rules 必须是数组");
        };
        for (index, rule) in rules.iter().enumerate() {
            let Some(rule) = rule.as_object() else {
                bail!("sing-box route.rules[{}] 必须是对象", index);
            };
            if let Some(rule_set) = rule.get("rule_set") {
                referenced_rule_sets.extend(
                    validate_rule_set_match(rule_set)
                        .with_context(|| format!("route.rules[{index}].rule_set"))?,
                );
            }
            let Some(action) = rule.get("action").and_then(Value::as_str) else {
                bail!("sing-box route.rules[{}] 缺少 action", index);
            };
            if !matches!(
                action,
                "route" | "bypass" | "reject" | "hijack-dns" | "route-options" | "sniff" | "resolve"
            ) {
                bail!("sing-box route.rules[{}].action 不受支持", index);
            }
            if let Some(outbound) = rule.get("outbound") {
                if !outbound.as_str().is_some_and(|outbound| !outbound.trim().is_empty()) {
                    bail!("sing-box route.rules[{}].outbound 必须是非空字符串", index);
                }
            }
            if action == "route"
                && !rule
                    .get("outbound")
                    .and_then(Value::as_str)
                    .is_some_and(|outbound| !outbound.trim().is_empty())
            {
                bail!("sing-box route.rules[{}] 的 route action 必须指定 outbound", index);
            }
        }
    }
    if let Some(rule_sets) = object.get("rule_set") {
        let Some(rule_sets) = rule_sets.as_array() else {
            bail!("sing-box route.rule_set 必须是数组");
        };
        let mut declared_tags = std::collections::HashSet::new();
        for (index, rule_set) in rule_sets.iter().enumerate() {
            let tags =
                validate_rule_set_definition(rule_set).with_context(|| format!("route.rule_set[{index}]"))?;
            for tag in tags {
                if !declared_tags.insert(tag.to_owned()) {
                    bail!("sing-box route.rule_set 存在重复 tag: {tag}");
                }
            }
        }
        for tag in referenced_rule_sets {
            if !declared_tags.contains(tag) {
                bail!("sing-box route.rules 引用了未定义的 rule_set: {tag}");
            }
        }
    } else if let Some(tag) = referenced_rule_sets.first() {
        bail!("sing-box route.rules 引用了未定义的 rule_set: {tag}");
    }
    Ok(())
}

fn normalize_clash_rules(rules: &str) -> Result<String> {
    if rules.len() > MAX_CUSTOM_RULE_CONFIG_BYTES {
        bail!("Clash rules 不能超过 {} KiB", MAX_CUSTOM_RULE_CONFIG_BYTES / 1024);
    }
    let mut rendered = Vec::new();
    let mut rule_positions = Vec::new();
    let mut match_position = None;
    for (line_index, source_line) in rules.lines().enumerate() {
        let line_number = line_index + 1;
        let line = source_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            rendered.push(line.to_owned());
            continue;
        }
        let rule = line.strip_prefix("- ").unwrap_or(line).trim();
        if rule.is_empty() || rule.starts_with('-') {
            bail!("Clash 第 {line_number} 行不是有效规则");
        }
        let fields = split_clash_fields(rule, line_number)?;
        if fields.len() < 2 || fields.iter().any(|field| field.trim().is_empty()) {
            bail!("Clash 第 {line_number} 行必须包含规则类型和策略");
        }
        let rule_type = fields[0].trim();
        if rule_type.is_empty()
            || !rule_type.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            bail!("Clash 第 {line_number} 行的规则类型无效");
        }
        let is_match = rule_type.eq_ignore_ascii_case("MATCH");
        if is_match {
            if fields.len() != 2 {
                bail!("Clash 第 {line_number} 行的 MATCH 规则只能包含一个策略");
            }
            if match_position.is_some() {
                bail!("Clash rules 只能包含一个 MATCH 规则");
            }
            match_position = Some(rule_positions.len());
        }
        rule_positions.push(rendered.len());
        rendered.push(format!("- {rule}"));
    }
    if rendered.is_empty() || rule_positions.is_empty() {
        bail!("Clash rules 不能为空");
    }
    if let (Some(match_index), Some(last_rule_index)) = (match_position, rule_positions.last()) {
        if rule_positions.get(match_index) != Some(last_rule_index) {
            bail!("Clash MATCH 规则必须放在最后");
        }
    }
    Ok(rendered.join("\n"))
}

fn split_clash_fields(rule: &str, line_number: usize) -> Result<Vec<&str>> {
    let mut depth = 0usize;
    let mut fields = Vec::new();
    let mut start = 0usize;
    for (index, ch) in rule.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    bail!("Clash 第 {line_number} 行括号不匹配");
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                fields.push(&rule[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        bail!("Clash 第 {line_number} 行括号不匹配");
    }
    fields.push(&rule[start..]);
    Ok(fields)
}

fn validate_rule_set_match(value: &Value) -> Result<Vec<&str>> {
    let mut tags = Vec::new();
    match value {
        Value::String(tag) if !tag.trim().is_empty() => tags.push(tag.as_str()),
        Value::Array(items) if !items.is_empty() => {
            for item in items {
                let Some(tag) = item.as_str() else {
                    bail!("rule_set 匹配项必须是字符串");
                };
                if tag.trim().is_empty() {
                    bail!("rule_set 匹配项不能为空");
                }
                tags.push(tag);
            }
        }
        _ => bail!("rule_set 匹配项必须是非空字符串或字符串数组"),
    }
    Ok(tags)
}

fn validate_rule_set_definition(value: &Value) -> Result<Vec<&str>> {
    let Some(object) = value.as_object() else {
        bail!("必须是对象");
    };
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(_) => bail!("type 必须是字符串"),
        // sing-box 允许内联 rule-set 省略 type。
        None if object.get("rules").is_some() => "inline",
        None => bail!("缺少 type"),
    };
    if !matches!(kind, "remote" | "local" | "inline") {
        bail!("type 必须是 remote、local 或 inline");
    }
    let Some(tag) = object.get("tag") else {
        bail!("缺少 tag");
    };
    let tags = match tag {
        Value::String(tag) if !tag.trim().is_empty() => vec![tag.as_str()],
        Value::Array(tags) if !tags.is_empty() => {
            let mut out = Vec::with_capacity(tags.len());
            for tag in tags {
                let Some(tag) = tag.as_str() else {
                    bail!("tag 数组只能包含字符串");
                };
                if tag.trim().is_empty() {
                    bail!("tag 不能为空");
                }
                out.push(tag);
            }
            out
        }
        _ => bail!("tag 必须是非空字符串或字符串数组"),
    };
    if let Some(format) = object.get("format") {
        if !format.as_str().is_some_and(|format| matches!(format, "source" | "binary")) {
            bail!("format 必须是 source 或 binary");
        }
    }
    match kind {
        "remote" => {
            require_nonempty_string(object, "url")?;
        }
        "local" => {
            require_nonempty_string(object, "path")?;
        }
        "inline" => {
            if !object.get("rules").is_some_and(Value::is_array) {
                bail!("inline rule-set 的 rules 必须是数组");
            }
        }
        _ => bail!("未知 rule-set type"),
    }
    Ok(tags)
}

fn require_nonempty_string<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{key} 必须是非空字符串"))
}

fn reject_removed_route_fields(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if matches!(key.as_str(), "geoip" | "geosite") {
                    bail!("{path}.{key} 已弃用，请改用 rule_set");
                }
                reject_removed_route_fields(child, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_removed_route_fields(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn remote_rule_set(tag: &str, url: &str) -> Value {
    json!({"type": "remote", "tag": tag, "format": "binary", "url": url, "update_interval": "1d"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: RuleMode) -> RuleConfig {
        RuleConfig { mode, ..RuleConfig::default() }
    }

    #[test]
    fn rule_modes_serialize_as_stable_lowercase_values() {
        assert_eq!(serde_json::to_string(&RuleMode::Smart).unwrap(), "\"smart\"");
        assert_eq!(serde_json::from_str::<RuleMode>("\"custom\"").unwrap(), RuleMode::Custom);
        assert!(serde_json::from_str::<RuleMode>("\"automatic\"").is_err());
    }

    #[test]
    fn smart_clash_rules_keep_supplied_order_and_put_match_last() {
        let rules = render_clash_rules(&config(RuleMode::Smart)).unwrap();
        assert!(rules.starts_with("# --- 局域网与私有网络直连 ---\n- GEOIP,LAN,DIRECT"));
        assert!(rules.contains("- DOMAIN-SUFFIX,openai.com,节点选择"));
        assert!(rules.contains("- DOMAIN-KEYWORD,gemini,节点选择"));
        assert!(rules.ends_with("# --- 最终兜底规则 ---\n- MATCH,节点选择"));
        validate_clash_rules(&rules).unwrap();
    }

    #[test]
    fn lite_and_global_clash_presets_keep_match_last() {
        let lite = render_clash_rules(&config(RuleMode::Lite)).unwrap();
        assert!(lite.contains("- GEOIP,CN,DIRECT"));
        assert!(!lite.contains("openai.com"));
        assert!(lite.ends_with("- MATCH,节点选择"));

        let global = render_clash_rules(&config(RuleMode::Global)).unwrap();
        assert!(!global.contains("GEOIP,CN"));
        assert!(global.ends_with("- MATCH,节点选择"));
    }

    #[test]
    fn clash_validator_rejects_match_before_later_rules() {
        assert!(validate_clash_rules("MATCH,节点选择\nGEOIP,CN,DIRECT").is_err());
        assert!(validate_clash_rules("GEOIP,CN,DIRECT\nMATCH,节点选择\nMATCH,DIRECT").is_err());
        assert!(validate_clash_rules("DOMAIN-SUFFIX,example.com,DIRECT\nMATCH,节点选择").is_ok());
    }

    #[test]
    fn singbox_presets_use_rule_sets_and_modern_route_fields() {
        let smart = render_singbox_route(&config(RuleMode::Smart)).unwrap();
        assert_eq!(smart["final"], "proxy");
        assert_eq!(smart["rule_set"].as_array().unwrap().len(), 2);
        assert!(smart["rules"].as_array().unwrap().iter().any(|rule| rule["ip_is_private"] == true));
        assert!(smart["rules"].as_array().unwrap().iter().any(|rule| rule["domain_keyword"][0] == "gemini"));
        assert!(smart["rules"].as_array().unwrap().iter().any(|rule| {
            rule["ip_cidr"].as_array().is_some_and(|cidrs| cidrs.iter().any(|cidr| cidr == "100.64.0.0/10"))
        }));
        assert!(smart["rules"].as_array().unwrap().iter().any(|rule| rule["rule_set"][0] == GEO_SITE_CN_TAG));
        assert!(smart["rules"].as_array().unwrap().iter().any(|rule| rule["rule_set"][0] == GEO_IP_CN_TAG));
        let smart_rules = smart["rules"].as_array().unwrap();
        let proxy_override = smart_rules.iter().position(|rule| rule["outbound"] == "proxy").unwrap();
        let china_rule = smart_rules.iter().position(|rule| rule["rule_set"][0] == GEO_SITE_CN_TAG).unwrap();
        assert!(proxy_override < china_rule);
        validate_singbox_route(&smart).unwrap();

        let lite = render_singbox_route(&config(RuleMode::Lite)).unwrap();
        assert!(!lite["rules"].as_array().unwrap().iter().any(|rule| rule.get("domain_keyword").is_some()));
        assert_eq!(lite["final"], "proxy");

        let global = render_singbox_route(&config(RuleMode::Global)).unwrap();
        assert!(global["rule_set"].as_array().unwrap().is_empty());
        assert_eq!(global["final"], "proxy");
    }

    #[test]
    fn custom_clash_yaml_rules_are_normalized_and_validated() {
        let mut custom = config(RuleMode::Custom);
        custom.custom.clash_rules = "DOMAIN-SUFFIX,example.com,DIRECT\nMATCH,节点选择".into();
        let rendered = render_clash_rules(&custom).unwrap();
        assert_eq!(rendered, "- DOMAIN-SUFFIX,example.com,DIRECT\n- MATCH,节点选择");
        custom.custom.singbox_route = json!({"rules": [], "rule_set": [], "final": "proxy"});
        custom.validate().unwrap();
    }

    #[test]
    fn custom_singbox_route_rejects_legacy_fields_and_bad_shapes() {
        assert!(validate_singbox_route(&json!({"rules": [{"geosite": ["cn"]}]})).is_err());
        assert!(validate_singbox_route(&json!({"rules": "bad"})).is_err());
        assert!(validate_singbox_route(&json!({"rule_set": [{"type": "remote", "tag": "cn", "format": "binary", "url": "https://rules.invalid/cn.srs"}], "rules": [{"rule_set": ["cn"], "action": "route", "outbound": "direct"}], "final": "proxy"})).is_ok());
        assert!(validate_singbox_route(&json!({"rules": [{"rule_set": ["missing"], "action": "route", "outbound": "direct"}], "final": "proxy"})).is_err());
        assert!(
            validate_singbox_route(&json!({"rules": [{"ip_is_private": true}], "final": "proxy"})).is_err()
        );
        assert!(validate_singbox_route(&json!({"rules": [], "rule_set": [], "final": ""})).is_err());
        assert!(validate_singbox_route(
            &json!({"rule_set": [{"tag": "inline-cn", "rules": []}], "final": "proxy"})
        )
        .is_ok());
    }
}
