use super::*;

// ---- panel write paths ----

/// Names all three causes. `--site` is the one input to this decision that
/// nothing about the request reveals: a hub started with
/// `--site https://198.51.100.7` refuses every provisioning call from an
/// otherwise valid https domain entry, and a tunnelled panel is refused until
/// one is given. `main` warns about the first at startup; this is for whoever
/// reads the panel rather than the journal.
pub(super) const PROVISIONING_DENIED: &str = "请通过 HTTPS 域名访问面板后添加或安装节点；\
     从隧道或回环地址进面板时，给 hub 加 --site 指定节点可达的域名；\
     --site 必须是 https:// 加域名，不能是 IP、不能带路径";

/// Every browser sends `Origin` with these writes, so its absence points at a
/// proxy clearing it. The panel judges from its own address bar and offers the
/// button, which leaves this as the only account of why the hub refuses.
pub(super) const ORIGIN_MISSING: &str = "请求没有带 Origin 头。浏览器都会发送它，多半是反向代理清掉了\
     （例如 proxy_set_header Origin \"\"），去掉那一行后再试";

/// Whether this origin is the hub's own machine, which is what a tunnel into the
/// panel leaves in the address bar. Nothing between that browser and the hub is
/// in the clear -- it is the loopback interface, or the tunnel's own encryption
/// -- so the entry is sound; what it lacks is an address a node could use, which
/// is why it counts only alongside `--site`.
pub(super) fn loopback_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else { return false };
    let Some(host) = url.host_str() else { return false };
    // host_str keeps the brackets an IPv6 literal is written with.
    let ip = host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>();
    matches!(url.scheme(), "http" | "https")
        && (host == "localhost" || host.ends_with(".localhost") || ip.is_ok_and(|ip| ip.is_loopback()))
}

pub(crate) fn https_domain(site: &str) -> Option<reqwest::Url> {
    let url = reqwest::Url::parse(site).ok()?;
    (url.scheme() == "https"
        && url.domain().is_some_and(|d| d != "localhost" && !d.ends_with(".localhost"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

/// Whether the browser sending this request is on an https domain entry, which
/// is the only address the panel may build install commands from.
///
/// The browser's own `Origin` answers it. Reconstructing the entry from `Host`
/// and `X-Forwarded-Proto` instead holds only where every proxy in front
/// forwards both, and the common ones do not: the aapanel and BT templates send
/// no `X-Forwarded-Proto`, a bare `proxy_pass` sends its own upstream address as
/// `Host` (as does Apache under `ProxyPreserveHost Off`), `$host` drops a port
/// that is not 443, and a TLS edge ahead of a plaintext hop leaves
/// `X-Forwarded-Proto: http`. Each of those refuses a panel that is in fact on
/// https, and none can be told apart here from a genuine plaintext entry.
/// `Origin` crosses all of them unchanged, and a page cannot forge its own.
///
/// `--site` is measured by the same rule, because it takes this origin's place
/// in the command: an IP or a path there is refused however the panel is
/// reached. It also answers for the one entry this origin cannot: a panel opened
/// over a tunnel reads `http://127.0.0.1:PORT`, which names no address a node
/// could reach, while `--site` names one and the tunnel carries the session
/// under its own encryption.
///
/// The error is the message the panel shows.
pub(super) fn provisioning_allowed(app: &App, headers: &HeaderMap) -> Result<(), &'static str> {
    if !app.site.is_empty() && https_domain(&app.site).is_none() {
        debug!("provisioning refused: --site {:?} is not an https domain entry", app.site);
        return Err(PROVISIONING_DENIED);
    }
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        debug!("provisioning refused: the request carries no readable Origin");
        return Err(ORIGIN_MISSING);
    };
    if https_domain(origin).is_none() && !(loopback_origin(origin) && !app.site.is_empty()) {
        debug!("provisioning refused: Origin {origin:?} is not an https domain entry");
        return Err(PROVISIONING_DENIED);
    }
    Ok(())
}

/// Range and sign limits every stored node must satisfy, or the reason it does
/// not. Shared because both writers must enforce them: the create path formerly
/// accepted a whole `Node` unchecked, leaving the values the update path refuses
/// reachable by another route, and an out-of-range reset day remained harmless
/// only because `period_start` clamps what it reads.
pub(super) fn node_limits(
    reset_day: Option<u32>,
    price: Option<f64>,
    limit: Option<i64>,
) -> Option<&'static str> {
    if reset_day.is_some_and(|d| !(1..=31).contains(&d)) {
        return Some("流量重置日要在 1 到 31 之间");
    }
    if price.is_some_and(|v| !v.is_finite() || v < 0.0) || limit.is_some_and(|v| v < 0) {
        return Some("价格和流量上限不能是负数");
    }
    None
}

/// A theme shows the group as a tab label or a section title, so it is held to a
/// length that fits one line on a 390 px phone.
pub(super) const MAX_GROUP: usize = 13;

/// Trims a group name, or refuses it. Refused rather than truncated: the panel
/// would otherwise report saved a name that is not the one stored.
pub(super) fn group_error(group: &mut String) -> Option<&'static str> {
    *group = group.trim().to_owned();
    if group.chars().count() > MAX_GROUP || group.chars().any(char::is_control) {
        return Some("分组名最多 13 个字，不能含控制字符");
    }
    None
}

/// Normalizes the currency and billing cycle, or names the one that cannot be
/// stored. The currency is held to the ISO 4217 form of three letters, the only
/// one `Intl.NumberFormat` accepts, so a theme may pass it on without guarding
/// against a throw. Each cycle length is stored in a single spelling, see
/// `cycle_name`.
pub(super) fn billing_error(
    currency: Option<&mut String>,
    cycle: Option<&mut String>,
) -> Option<&'static str> {
    if let Some(code) = currency {
        *code = code.trim().to_ascii_uppercase();
        if !(code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())) {
            return Some("货币要填三个字母的代码，如 USD、CNY、HKD");
        }
    }
    if let Some(cycle) = cycle.filter(|c| *c != "once") {
        let Some(months) = crate::cycle_months(cycle) else {
            return Some("付款周期要是整月，在 1 个月到 100 年之间");
        };
        *cycle = crate::cycle_name(months);
    }
    None
}

/// Normalizes a patch, or names the first value that cannot be stored. The one
/// check both the single and the batch write pass through, so the two accept
/// exactly the same values.
pub(super) fn patch_error(node: &mut NodePatch) -> Option<&'static str> {
    if let Some(message) = node.name.as_mut().and_then(name_error) {
        return Some(message);
    }
    if let Some(group) = &mut node.group {
        if let Some(message) = group_error(group) {
            return Some(message);
        }
    }
    // Refused rather than truncated, as a group name is.
    if let Some(text) = &mut node.public_remark {
        *text = text.trim().to_owned();
        if text.chars().count() > MAX_PUBLIC_REMARK || text.chars().any(char::is_control) {
            return Some("公开备注最多 100 个字，不能含控制字符");
        }
    }
    node_limits(node.traffic_reset_day, node.price, node.traffic_limit)
        .or_else(|| billing_error(node.currency.as_mut(), node.billing_cycle.as_mut()))
        .or_else(|| pins(node))
}

/// Normalizes the values set by hand, or names the one that cannot stand. Each
/// takes the place of an automatic value, so it is held to what that value would
/// have to be: the country to the rule a looked-up one passes, as both reach the
/// status page; an address to its own family, stored canonical so the panel's
/// search matches however it was typed.
pub(super) fn pins(node: &mut NodePatch) -> Option<&'static str> {
    if let Some(cc) = &mut node.country_pin {
        *cc = cc.trim().to_ascii_uppercase();
        if !cc.is_empty() && !(cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase())) {
            return Some("国家要填两个字母的代码，留空则自动识别");
        }
    }
    for (pin, v6) in [(&mut node.ipv4_pin, false), (&mut node.ipv6_pin, true)] {
        let Some(pin) = pin else { continue };
        let typed = pin.trim();
        let parsed = if v6 {
            typed.parse::<Ipv6Addr>().map(IpAddr::V6)
        } else {
            typed.parse::<Ipv4Addr>().map(IpAddr::V4)
        };
        *pin = match parsed {
            Ok(ip) => ip.to_string(),
            Err(_) if typed.is_empty() => String::new(),
            Err(_) if v6 => return Some("IPv6 要填一个 IPv6 地址，或者留空"),
            Err(_) => return Some("IPv4 要填一个 IPv4 地址，或者留空"),
        };
    }
    None
}

/// Node and probe names, as a panel row, a theme card and a chart legend show
/// them. Refused rather than truncated, as a group name is.
pub(super) const MAX_NAME: usize = 64;

/// Trims a node name, or refuses it.
pub(super) fn name_error(name: &mut String) -> Option<&'static str> {
    *name = name.trim().to_owned();
    if name.is_empty() {
        return Some("请填写节点名称");
    }
    if name.chars().count() > MAX_NAME {
        return Some("节点名称最多 64 个字");
    }
    None
}

/// A public remark goes to every visitor in every frame, every two seconds, where
/// a node's public view is about 1 KB; a hundred CJK characters add 300 bytes.
pub(super) const MAX_PUBLIC_REMARK: usize = 100;
