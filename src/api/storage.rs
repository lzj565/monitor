use super::*;

// ---- the database itself ----

/// The largest single request the two upload routes accept, and the reason they
/// sit outside the router's 64 KiB body limit. It is twice the 4 MiB the panel
/// sends, so the chunk size remains the panel's concern alone and requires no
/// negotiated handshake.
///
/// **This, not the two ceilings below, is what a reverse proxy must pass.** A
/// backup of any size arrives 4 MiB at a time, so `client_max_body_size` no
/// longer tracks the size of the database.
pub const MAX_CHUNK: usize = 8 * 1024 * 1024;

/// Whole-file ceilings, one per route, checked against the declared `total` on
/// the first request rather than by counting bytes as they arrive, so an
/// oversized upload is refused before a byte is sent.
///
/// The backup ceiling is set where it is because restoring holds the connection
/// every read and write passes through: at the measured ~40 MB/s that is roughly
/// 6.5 seconds during which the panel and the public page also wait. Database
/// sizes reachable with a few hundred nodes sit two orders of magnitude below
/// it.
pub const MAX_RESTORE: u64 = 1024 * 1024 * 1024;
pub const MAX_THEME: u64 = 32 * 1024 * 1024;

/// One request of an upload: `total` is the whole file, `offset` where this piece
/// belongs within it.
///
/// The upload ID binds a unique file to one session and declared total; its
/// current length is the next accepted offset. A piece continues an upload only if
/// it begins exactly where the last ended, `offset = 0` truncates whatever an
/// interrupted attempt left behind, and nothing is ever left to collect.
#[derive(Clone, Deserialize)]
pub struct Chunk {
    pub(super) upload_id: String,
    pub(super) offset: u64,
    pub(super) total: u64,
}

/// Buffer at most one bounded chunk before writing. The worker owns the
/// upload lock and file, so cancellation cannot unlock a pending disk write.
async fn chunk_bytes(chunk: &Chunk, max: u64, body: axum::body::Body) -> anyhow::Result<axum::body::Bytes> {
    use anyhow::Context;
    if chunk.total == 0 || chunk.total > max {
        refuse!("文件必须在 1 字节到 {} MiB 之间", max / 1024 / 1024);
    }
    if chunk.offset > chunk.total {
        refuse!("分片位置越过了文件末尾");
    }
    let bytes = axum::body::to_bytes(body, MAX_CHUNK)
        .await
        .context(crate::Shown("上传中断或分片过大，请重新上传".into()))?;
    if chunk.offset + bytes.len() as u64 > chunk.total {
        refuse!("这一片超出了声明的文件大小");
    }
    Ok(bytes)
}

fn write_chunk(path: &std::path::Path, chunk: &Chunk, bytes: &[u8]) -> anyhow::Result<u64> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    if chunk.offset == 0 {
        options.write(true).create(true).truncate(true);
    } else {
        options.append(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            refuse!("这次上传已经不在了，请从头开始")
        }
        Err(error) => return Err(error.into()),
    };
    let already = file.metadata()?.len();
    if already != chunk.offset {
        refuse!("分片接不上：已经收到 {already} 字节，这一片却从 {} 开始", chunk.offset);
    }
    if let Err(error) = file.write_all(bytes) {
        let _ = file.set_len(chunk.offset);
        return Err(error.into());
    }
    Ok(chunk.offset + bytes.len() as u64)
}

async fn receive_upload(
    upload: tokio::sync::OwnedMutexGuard<crate::uploads::Upload>,
    chunk: Chunk,
    max: u64,
    body: axum::body::Body,
) -> anyhow::Result<(tokio::sync::OwnedMutexGuard<crate::uploads::Upload>, u64)> {
    let bytes = chunk_bytes(&chunk, max, body).await?;
    crate::blocking::run(move || {
        let received = write_chunk(&upload.path, &chunk, &bytes)?;
        anyhow::Ok((upload, received))
    })
    .await?
}

#[cfg(test)]
pub(super) async fn receive(
    path: &str,
    chunk: &Chunk,
    max: u64,
    body: axum::body::Body,
) -> anyhow::Result<u64> {
    let bytes = chunk_bytes(chunk, max, body).await?;
    let path = std::path::PathBuf::from(path);
    let chunk = chunk.clone();
    crate::blocking::run(move || write_chunk(&path, &chunk, &bytes)).await?
}

/// A scratch file beside the database, so the copy lands on the same filesystem
/// the database has room on. The random component keeps two concurrent calls
/// apart, since `VACUUM INTO` refuses an existing file.
pub(super) fn scratch_path(app: &App, kind: &str) -> String {
    format!("{}.{kind}-{}.tmp", app.db.file(), &random_token()[..16])
}

/// The data page's figures.
///
/// Off the runtime, like the three routes below: `stats` counts every row of
/// `metric` and `ping_record` -- both WITHOUT ROWID, so each count is a full
/// index scan -- holding the connection the agents report through throughout. At
/// 2.2M rows that is 127 ms during which the public page and every agent report
/// also wait, growing with `retention_days`.
pub async fn db_stats(_: Admin, State(app): State<Shared>) -> Response {
    match crate::blocking::run(move || app.db.stats()).await {
        Ok(Ok(stats)) => Json(stats).into_response(),
        Ok(Err(e)) => fail(e),
        Err(e) => fail(anyhow::anyhow!(e)),
    }
}

/// Returns a compact copy of the whole database.
///
/// The copy is written beside the live file and then unlinked while still open,
/// so it exists only for the duration of this response: a client that
/// disconnects partway through leaves nothing behind, and nothing on disk
/// outlives the download.
pub async fn db_backup(_: Admin, State(app): State<Shared>) -> Response {
    let path = scratch_path(&app, "backup");
    // Off the runtime: this reads the entire database, through a connection of
    // its own, so the agents' writes continue meanwhile.
    let copied = {
        let (app, path) = (app.clone(), path.clone());
        crate::blocking::run(move || app.db.backup_into(&path)).await
    };
    if let Err(e) = copied.map_err(|e| anyhow::anyhow!(e)).and_then(|r| r) {
        let _ = std::fs::remove_file(&path);
        return fail(e);
    }
    let opened = tokio::fs::File::open(&path).await;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&path);
    match opened {
        Ok(file) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                (header::CONTENT_LENGTH, size.to_string()),
                // The entire credential store: no shared cache may retain a copy.
                (header::CACHE_CONTROL, "no-store".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"monitor-{}.db\"", Local::now().format("%Y%m%d-%H%M%S")),
                ),
            ],
            axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)),
        )
            .into_response(),
        Err(e) => fail(e),
    }
}

/// Replaces the live database with an uploaded backup, one chunk per request.
///
/// The upload streams to a file beside the database and is validated in full
/// before a single page is copied; see `Db::check_backup`. Afterwards every
/// session in the restored file is dropped and the caller is issued a new one: a
/// backup carries the session rows it held when taken, and restoring it must not
/// revive logged-out sessions.
pub async fn db_restore(
    _: Admin,
    State(app): State<Shared>,
    Query(chunk): Query<Chunk>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let Some(owner) = current_session(&headers) else {
        return answer(StatusCode::UNAUTHORIZED, "登录已失效，请重新登录");
    };
    let path = std::path::PathBuf::from(format!(
        "{}.upload-{}-{}",
        app.db.file(),
        chunk.upload_id,
        &random_token()[..16]
    ));
    let upload = match app.uploads.begin(&chunk.upload_id, owner, "restore", chunk.total, chunk.offset, path)
    {
        Ok(upload) => upload,
        Err(error) => return fail(error),
    };
    let (mut upload, received) = match receive_upload(upload, chunk.clone(), MAX_RESTORE, body).await {
        Ok(result) => result,
        Err(error) => return fail(error),
    };
    if received < chunk.total {
        return Json(json!({"received": received})).into_response();
    }
    let Ok(restore_guard) = app.restore_gate.clone().try_lock_owned() else {
        upload.sealed = true;
        app.uploads.finish(&chunk.upload_id);
        return answer(StatusCode::CONFLICT, "已有备份正在恢复，请稍后重试");
    };
    upload.sealed = true;
    let path = upload.path.to_string_lossy().into_owned();
    // Both locks move onto the worker: cancelling the HTTP request cannot let
    // another restore or writer race the validation/copy still in progress.
    let result = crate::blocking::run(move || {
        let _restore_guard = restore_guard;
        let _upload = upload;
        let outcome = (|| {
            app.db.check_backup(&path)?;
            app.db.restore_from(&path)?;
            // Trust in the local network must be confirmed again after restore.
            app.db.set("origin_cdn_only", "off")?;
            app.agents.write().unwrap_or_else(|e| e.into_inner()).clear();
            app.readings.lock().unwrap_or_else(|e| e.into_inner()).clear();
            invalidate_snapshot(&app);
            app.db.drop_all_sessions()?;
            issue_session(&app, &headers)
        })();
        app.uploads.finish(&chunk.upload_id);
        outcome
    })
    .await;
    match result {
        Ok(Ok(cookie)) => with_cookies(Json(json!({"ok": true})), [cookie]),
        Ok(Err(error)) | Err(error) => fail(error),
    }
}

/// Drops history beyond the retention window and rebuilds the file around what
/// remains, which is the only way SQLite returns the space to the filesystem.
pub async fn db_vacuum(_: Admin, State(app): State<Shared>) -> Response {
    let app = app.clone();
    // A rebuild of the whole file, holding the connection the agents write
    // through, so it belongs on a blocking thread.
    let done = crate::blocking::run(move || {
        let keep = app.db.retention_days();
        let pruned = app.db.prune(keep)?;
        app.db.vacuum().map(|freed| json!({"pruned": pruned, "freed": freed}))
    })
    .await;
    match done.map_err(|e| anyhow::anyhow!(e)).and_then(|r| r) {
        Ok(result) => Json(result).into_response(),
        Err(e) => fail(e),
    }
}

/// Installs an uploaded theme archive, one chunk per request.
///
/// The archive lands in the themes directory under a name `valid_short` rejects,
/// so a partial upload is invisible to both the theme list and the public page.
/// Installation is performed by `frontend::install`, which unpacks to a staging
/// directory and publishes with a rename: the switch is atomic, and the page is
/// never served from a partially written directory.
pub async fn upload_theme(
    _: Admin,
    State(app): State<Shared>,
    Query(chunk): Query<Chunk>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let Some(owner) = current_session(&headers) else {
        return answer(StatusCode::UNAUTHORIZED, "登录已失效，请重新登录");
    };
    let path = app.themes.join(format!(".upload-{}-{}.tar.gz", chunk.upload_id, &random_token()[..16]));
    let upload = match app.uploads.begin(&chunk.upload_id, owner, "theme", chunk.total, chunk.offset, path) {
        Ok(upload) => upload,
        Err(error) => return fail(error),
    };
    let (mut upload, received) = match receive_upload(upload, chunk.clone(), MAX_THEME, body).await {
        Ok(result) => result,
        Err(error) => return fail(error),
    };
    if received < chunk.total {
        return Json(json!({"received": received})).into_response();
    }
    upload.sealed = true;
    let installed = crate::blocking::run(move || {
        let upload = upload;
        let result = (|| crate::frontend::install(&app.themes, std::fs::File::open(&upload.path)?, None))();
        app.uploads.finish(&chunk.upload_id);
        result
    })
    .await;
    match installed {
        Ok(Ok(theme)) => Json(json!({"theme": theme})).into_response(),
        Ok(Err(error)) | Err(error) => fail(error),
    }
}
