use super::*;

/// The asset a theme repository publishes, and the only name the hub fetches: the
/// same `theme.tar.gz` the upload button accepts.
pub(super) const ARCHIVE: &str = "theme.tar.gz";

/// How long a release lookup stands before the panel asks GitHub again, and how
/// long a failed one does. The short retry keeps one unreachable moment from
/// hiding an update for the rest of the day; the long one keeps a panel left
/// open on a screen to four lookups a day, well inside the 60 per hour an
/// unauthenticated caller is allowed.
pub(super) const RELEASES_FRESH: i64 = 6 * 3600;
pub(super) const RELEASES_RETRY: i64 = 600;

/// What is running here, and what is published. The agent's own version travels
/// in its hello and is already in the node list, so the panel compares the two
/// itself and names the nodes to upgrade.
///
/// Admin-only, and deliberately not part of `/api/me`: that route answers
/// anonymous callers, to whom the running hub version is not disclosed. Nothing
/// is fetched until an administrator opens the panel, so a hub whose panel is
/// never opened makes no outbound request.
pub async fn versions(_: Admin, State(app): State<Shared>) -> Response {
    let cached = app.releases.lock().unwrap().clone();
    let latest = if fresh_enough(&cached, Utc::now().timestamp()) {
        cached
    } else {
        // Both at once: one round trip of latency rather than two, and neither
        // lookup depends on the other.
        let (hub, agent) =
            tokio::join!(latest_tag(&app, crate::HUB_REPO), latest_tag(&app, crate::AGENT_REPO));
        let read = crate::Releases {
            read_at: Utc::now().timestamp(),
            hub: hub.unwrap_or_default(),
            agent: agent.unwrap_or_default(),
        };
        *app.releases.lock().unwrap() = read.clone();
        read
    };
    let notice_app = app.clone();
    let notice = crate::blocking::run(move || notice_app.db.get("update_notice").as_deref() != Some("off"))
        .await
        .unwrap_or(false);
    Json(json!({
        "hub": env!("CARGO_PKG_VERSION"),
        // Empty where GitHub could not be reached, which the panel renders as no
        // update rather than as an error: a hub on a network that cannot reach
        // github.com is a supported deployment, not a fault to report.
        "hub_latest": latest.hub,
        "agent_latest": latest.agent,
        // Whether the navigation marks an update. It governs the mark alone: the
        // lookup runs either way, so the update page still answers when opened.
        "notice": notice,
    }))
    .into_response()
}

/// Whether the cached lookup still answers. One that returned nothing is held
/// for [`RELEASES_RETRY`] instead, so a single unreachable moment does not hide
/// an update for the rest of the day.
pub(super) fn fresh_enough(cached: &crate::Releases, now: i64) -> bool {
    let holds =
        if cached.hub.is_empty() || cached.agent.is_empty() { RELEASES_RETRY } else { RELEASES_FRESH };
    cached.read_at != 0 && now - cached.read_at < holds
}

/// The tag of a repository's latest release, without its leading `v`, or None
/// where GitHub could not be read, which costs only the update notice.
pub(super) async fn latest_tag(app: &App, repo: &str) -> Option<String> {
    let tag = latest_release(app, repo).await.ok()?.tag_name;
    Some(tag.strip_prefix('v').unwrap_or(&tag).to_owned())
}

/// The latest release of `owner/repo`. Unauthenticated: 60 requests per hour from
/// this address. GitHub returns 403 without a User-Agent. Never through the
/// panel's GitHub proxy, which most mirrors provide for release downloads alone.
pub(super) async fn latest_release(app: &App, repo: &str) -> reqwest::Result<Release> {
    app.http
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header(header::USER_AGENT, "monitor-hub")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

#[derive(Deserialize)]
pub(super) struct Release {
    pub(super) tag_name: String,
    #[serde(default)]
    pub(super) assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(super) struct Asset {
    pub(super) name: String,
}

/// The `<owner>/<repo>` a theme's `url` names, where it names a GitHub repository
/// at all.
///
/// An allowlist rather than a filter. Every address the update path fetches is
/// constructed from these two strings, so nothing in a manifest can direct the
/// hub at a host it did not choose, which is why no private-address check is
/// needed here. The only host that is not github.com is the GitHub proxy in the
/// panel's settings, configured by the operator and already used by the agent
/// relay.
pub(super) fn github_repo(url: &str) -> Option<(&str, &str)> {
    let (owner, rest) = url.strip_prefix("https://github.com/")?.split_once('/')?;
    // A link to a branch or a file is still a link to the repository, as is the
    // `?tab=readme-ov-file` or `#readme` a browser's address bar often carries;
    // neither part reaches the addresses built from the result.
    let repo = rest.split(['/', '?', '#']).next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    (path_segment(owner) && path_segment(repo)).then_some((owner, repo))
}

/// One URL path segment the hub will build a github.com address from: nothing
/// that opens a new segment, and nothing that escapes the current one.
pub(super) fn path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

/// Reinstalls one theme from the latest GitHub release of the repository its
/// manifest names.
///
/// The manifest supplies `<owner>/<repo>` and nothing more: the release is read
/// from api.github.com and the archive from github.com, both at addresses the hub
/// constructs itself, so no URL from the theme is ever followed. The installed
/// version is compared against the release tag first, which is all most
/// invocations do, making this also the check-for-updates action.
pub async fn update_theme(_: Admin, State(app): State<Shared>, Path(short): Path<String>) -> Response {
    match update(&app, &short).await {
        Ok((updated, version)) => Json(json!({"updated": updated, "version": version})).into_response(),
        Err(e) => fail(e),
    }
}

pub(super) async fn update(app: &App, short: &str) -> Result<(bool, String), anyhow::Error> {
    let Some(installed) = crate::blocking::in_place(|| crate::frontend::themes(app))?
        .into_iter()
        .find(|theme| theme.short == short)
    else {
        refuse!("没有这个主题");
    };
    let Some((owner, repo)) = github_repo(&installed.url) else {
        refuse!("这个主题的 url 不是 https://github.com/<owner>/<repo>，只能手动上传新包");
    };
    let release = latest_theme(app, owner, repo).await?;

    // Tags read `v1.2.3` while manifests carry `1.2.3`. Equal means up to date;
    // anything else is installed, including a deliberate downgrade, since the
    // release is what the author published.
    let tag = &release.tag_name;
    if tag.strip_prefix('v').unwrap_or(tag) == installed.version {
        return Ok((false, installed.version));
    }
    // Constrained to the theme it may replace. The built-in theme has no
    // directory until this runs: updating it writes one, which then serves in
    // place of the embedded copy until it is deleted.
    let theme = fetch_theme(app, owner, repo, &release, Some(short)).await?;
    Ok((true, theme.version))
}

/// The thumbnail the theme list displays, where the theme provides one; the list
/// reports which do.
pub async fn theme_preview(_: Admin, State(app): State<Shared>, Path(short): Path<String>) -> Response {
    crate::blocking::response(move || {
        match crate::frontend::preview(&app.themes, &short) {
            // Not cached: reinstalling a theme under the same name also replaces the
            // image, and this is a panel-only request for a local file.
            Some(png) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-cache")], png)
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    })
    .await
}

/// Deletes an installed theme. Deleting the one in use is permitted: the public
/// page falls back to the built-in theme from the next request, the same path a
/// broken theme already takes, and leaving the setting intact means reinstalling
/// the theme restores it.
pub async fn delete_theme(_: Admin, State(app): State<Shared>, Path(short): Path<String>) -> Response {
    crate::blocking::response(move || match crate::frontend::remove(&app.themes, &short) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => fail(e),
    })
    .await
}

/// Where a theme's saved settings live. Keyed by `short` in the database rather
/// than stored beside the theme, so updating, reinstalling or deleting the
/// theme leaves them in place, and a backup carries them.
pub(super) fn theme_config_key(short: &str) -> String {
    format!("theme_config:{short}")
}

/// The settings saved for one theme in the panel: only the fields changed from
/// the defaults its `theme.json` declares, which the theme fills in itself.
/// Any theme may be named, installed or not, so a theme under development
/// reads its own settings from whichever hub it proxies to.
///
/// Anonymous, under the same condition as `/api/nodes`. One request is one
/// primary-key read answering at most 64 KiB, the router's body limit having
/// bounded the write. That is the cost class of `/api/me`, so there is no gate:
/// on a three-core hub (debug build), 120 concurrent requests for a 60 KiB
/// value held the panel's `/api/nodes` at a 230 ms median, against 160 ms
/// under the same load on `/api/me`.
///
/// A failed read answers 500 rather than `{}`: the panel saves on top of what
/// it reads, so an empty answer would erase every saved override.
pub async fn theme_config(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(short): Path<String>,
) -> Response {
    crate::blocking::response(move || {
        if !authed(&app, &headers) && !app.public_page() {
            return answer(StatusCode::UNAUTHORIZED, "需要登录后查看");
        }
        if !crate::frontend::valid_short(&short) {
            return StatusCode::NOT_FOUND.into_response();
        }
        match app.db.lookup(&theme_config_key(&short)) {
            Ok(saved) => {
                let saved = saved.unwrap_or_else(|| "{}".into());
                ([(header::CONTENT_TYPE, "application/json")], saved).into_response()
            }
            Err(e) => fail(e),
        }
    })
    .await
}

/// Replaces a theme's saved settings. Values are not checked against the
/// theme's declared fields: the theme must validate what it reads regardless,
/// since a value saved under one version of the theme meets the next.
pub async fn save_theme_config(
    _: Admin,
    State(app): State<Shared>,
    Path(short): Path<String>,
    Json(values): Json<serde_json::Map<String, Value>>,
) -> Response {
    crate::blocking::response(move || {
        if !crate::frontend::valid_short(&short) || !crate::frontend::selectable(&app, &short) {
            return bad("主题没有安装");
        }
        match app.db.set(&theme_config_key(&short), &Value::Object(values).to_string()) {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => fail(e),
        }
    })
    .await
}

pub async fn themes(_: Admin, State(app): State<Shared>) -> Response {
    crate::blocking::response(move || {
        match crate::frontend::themes(&app) {
            Ok(mut themes) => {
                // Reads each image to answer, as serving it would: a handful of
                // themes, each image capped at 8 MiB.
                for theme in &mut themes {
                    theme.preview = crate::frontend::preview(&app.themes, &theme.short).is_some();
                }
                Json(json!({"themes": themes})).into_response()
            }
            Err(e) => fail(e),
        }
    })
    .await
}

/// Installs a theme from the latest release of a GitHub repository the
/// administrator pastes, in place of downloading its `theme.tar.gz` and
/// uploading it.
///
/// The trust is that of an upload: either way the administrator vouches for the
/// repository. The address is read as an update reads a manifest's `url`, with
/// only `<owner>/<repo>` taken from it, so the hub still fetches from no host but
/// GitHub and the configured proxy.
pub async fn install_theme(_: Admin, State(app): State<Shared>, Json(body): Json<Repository>) -> Response {
    let url = body.url.trim();
    // A bare `github.com/...`, the form addresses are often passed along in.
    let url = if url.starts_with("github.com/") { format!("https://{url}") } else { url.to_owned() };
    let Some((owner, repo)) = github_repo(&url) else {
        return bad("填主题的 GitHub 仓库地址，形如 https://github.com/作者/仓库");
    };
    let release = match latest_theme(&app, owner, repo).await {
        Ok(release) => release,
        Err(e) => return fail(e),
    };
    match fetch_theme(&app, owner, repo, &release, None).await {
        Ok(theme) => Json(json!({"theme": theme})).into_response(),
        Err(e) => fail(e),
    }
}

#[derive(Deserialize)]
pub struct Repository {
    url: String,
}

/// The latest release of a theme's repository, with GitHub's refusal put into
/// words the panel can show.
async fn latest_theme(app: &App, owner: &str, repo: &str) -> Result<Release, anyhow::Error> {
    use anyhow::Context;

    latest_release(app, &format!("{owner}/{repo}")).await.or_else(|e| {
        let why = match e.status().map(|s| s.as_u16()) {
            None if e.is_decode() => "GitHub 的回复无法识别，稍后再试",
            // A private repository answers the same as a missing one.
            Some(404) => "仓库不存在，或者还没有正式 release",
            // Unauthenticated callers get 60 requests an hour per address.
            Some(403 | 429) => "GitHub 限制了这台机器的请求次数，过一小时再试",
            Some(_) => "GitHub 接口出错，稍后再试",
            None => "hub 连不上 api.github.com，检查它的网络",
        };
        Err(e).context(crate::Shown(format!("读不到 {owner}/{repo} 的最新 release：{why}")))
    })
}

/// Downloads the `theme.tar.gz` of `release` and installs it; `expect` names
/// the theme it must replace, as for [`crate::frontend::install`].
async fn fetch_theme(
    app: &App,
    owner: &str,
    repo: &str,
    release: &Release,
    expect: Option<&str>,
) -> Result<crate::frontend::Theme, anyhow::Error> {
    use anyhow::Context;

    let tag = &release.tag_name;
    if !path_segment(tag) {
        refuse!("release 的 tag {tag:?} 不能出现在下载地址里");
    }
    // Checked here rather than by downloading and reading a 404: the asset name is
    // the contract, and stating so is the entire error message.
    if !release.assets.iter().any(|asset| asset.name == ARCHIVE) {
        refuse!("release {tag} 里没有 {ARCHIVE}");
    }

    // Through the panel's GitHub proxy when one is configured, the archive being
    // the part a blocked network cannot reach. The API call in `latest_theme` is
    // not proxied: most proxies front only releases, and a hub that cannot read
    // the tag still has the upload path.
    let direct = format!("https://github.com/{owner}/{repo}/releases/download/{tag}/{ARCHIVE}");
    let url = crate::proxied(app, direct.clone());
    let proxy = url != direct;
    let unreachable = || {
        crate::Shown(if proxy {
            format!("经 GitHub 代理下载 {ARCHIVE} 失败，换一个代理，或清空代理让 hub 直连")
        } else {
            format!("下载 {ARCHIVE} 失败：hub 连不上 github.com 时，在设置里填 GitHub 代理")
        })
    };
    let response = app
        .http
        .get(url)
        // With the release lookup's 15 s, this keeps the request inside the 100 s
        // Cloudflare waits for an origin before answering 524 itself: a download
        // too slow to finish is then reported as one, naming the proxy setting,
        // rather than as a hub that did not respond.
        .timeout(std::time::Duration::from_secs(75))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(unreachable)?;
    // The transfer stops at Content-Length, so checking it checks the body: a
    // header understating the archive cannot make more arrive. GitHub always
    // sends one; a proxy that omits it is refused rather than read unbounded.
    match response.content_length() {
        Some(size) if size <= MAX_THEME => {}
        Some(size) => {
            refuse!("主题包 {} MiB，超过 {} MiB 的上限", size / 1024 / 1024, MAX_THEME / 1024 / 1024)
        }
        None => refuse!("下载没有给出大小，无法确认它在 {} MiB 以内", MAX_THEME / 1024 / 1024),
    }
    let archive = response.bytes().await.with_context(unreachable)?;
    // Checked here as well as in `unpack`, whose answer is written for an upload.
    // A proxy answers with a page of its own -- a block notice, a sign-in wall --
    // under a 200.
    if !archive.starts_with(&crate::frontend::GZIP_MAGIC) {
        if proxy {
            refuse!("GitHub 代理返回的不是主题包，换一个代理，或清空代理让 hub 直连");
        }
        refuse!("release {tag} 里的 {ARCHIVE} 不是 gzip 格式，包本身有问题，请联系主题作者");
    }

    // The same unpacking, validation and atomic replace an upload undergoes.
    let (themes, expect) = (app.themes.clone(), expect.map(str::to_owned));
    crate::blocking::run(move || {
        crate::frontend::install(&themes, std::io::Cursor::new(archive), expect.as_deref())
    })
    .await?
}
