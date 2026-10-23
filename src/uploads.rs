//! Upload identities are scoped to a session and a route. Each identity has
//! its own file and lock, retained through validation and publication.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

const TTL: Duration = Duration::from_secs(3600);
const MAX_UPLOADS: usize = 8;

#[derive(Default)]
pub struct Registry(Mutex<HashMap<String, Arc<AsyncMutex<Upload>>>>);

pub struct Upload {
    pub path: PathBuf,
    owner: String,
    kind: &'static str,
    total: u64,
    touched: Instant,
    pub sealed: bool,
}

impl Drop for Upload {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

impl Registry {
    pub fn begin(
        &self,
        id: &str,
        owner: String,
        kind: &'static str,
        total: u64,
        offset: u64,
        path: PathBuf,
    ) -> Result<OwnedMutexGuard<Upload>> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            refuse!("上传 ID 必须是 32 位十六进制字符串，请重新上传");
        }
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::prune(&mut entries);
        let entry = match entries.get(id) {
            Some(entry) => entry.clone(),
            None => {
                if offset != 0 {
                    refuse!("这次上传已经不在了，请从头开始");
                }
                if entries.len() >= MAX_UPLOADS {
                    refuse!("同时上传的文件过多，请稍后重试");
                }
                let entry = Arc::new(AsyncMutex::new(Upload {
                    path,
                    owner: owner.clone(),
                    kind,
                    total,
                    touched: Instant::now(),
                    sealed: false,
                }));
                entries.insert(id.to_owned(), entry.clone());
                entry
            }
        };
        let Ok(mut upload) = entry.try_lock_owned() else {
            refuse!("同一上传仍有分片正在处理，请稍后重试");
        };
        if upload.owner != owner || upload.kind != kind || upload.total != total || upload.sealed {
            refuse!("上传会话、文件类型或大小不匹配，请重新上传");
        }
        upload.touched = Instant::now();
        Ok(upload)
    }

    fn prune(entries: &mut HashMap<String, Arc<AsyncMutex<Upload>>>) {
        entries.retain(|_, entry| {
            entry.try_lock().map_or(true, |upload| !upload.sealed && upload.touched.elapsed() < TTL)
        });
    }

    pub fn cleanup(&self) {
        Self::prune(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Collect files left by a process crash, while excluding every live
    /// upload. A locked entry postpones the scan rather than guessing its path.
    pub fn cleanup_files(&self, directory: &Path, prefix: &str, suffix: &str) {
        let entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut active = HashSet::new();
        for entry in entries.values() {
            let Ok(upload) = entry.try_lock() else { return };
            active.insert(upload.path.clone());
        }
        drop(entries);
        let Ok(files) = std::fs::read_dir(directory) else { return };
        for file in files.flatten() {
            let name = file.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(identity) = name.strip_prefix(prefix).and_then(|name| name.strip_suffix(suffix)) else {
                continue;
            };
            let Some((id, nonce)) = identity.split_once('-') else { continue };
            if id.len() != 32
                || nonce.len() != 16
                || !id.bytes().chain(nonce.bytes()).all(|b| b.is_ascii_hexdigit())
                || active.contains(&file.path())
            {
                continue;
            }
            let Ok(metadata) = file.metadata() else { continue };
            if metadata.is_file()
                && metadata.modified().ok().and_then(|time| time.elapsed().ok()).is_some_and(|age| age >= TTL)
            {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }

    /// Called by the worker after finalization, including failed validation.
    /// The guard still owns the entry until its file handles have closed.
    pub fn finish(&self, id: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_are_isolated_bound_and_locked_through_finalization() {
        let registry = Registry::default();
        let begin = |id: &str, owner: &str, total, offset| {
            registry.begin(id, owner.into(), "restore", total, offset, std::env::temp_dir().join(id))
        };
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let mut first = begin(a, "alice", 8, 0).unwrap();
        let second = begin(b, "bob", 8, 0).unwrap();
        assert_ne!(first.path, second.path);
        assert!(begin(a, "alice", 8, 0).is_err(), "an open writer prevents another chunk");
        first.sealed = true;
        assert!(begin(a, "alice", 8, 4).is_err(), "validation retains the writer lock");
        drop(first);
        assert!(begin(a, "alice", 8, 4).is_err(), "a completed upload cannot resume");
        drop(second);
        assert!(begin(b, "alice", 8, 4).is_err(), "another session cannot take an upload");
        assert!(begin(b, "bob", 9, 4).is_err(), "total cannot change between chunks");
        assert!(registry.begin(b, "bob".into(), "theme", 8, 4, PathBuf::new()).is_err());
        assert!(begin("../../bad", "alice", 8, 0).is_err());
    }

    #[test]
    fn stale_uploads_are_collected_with_their_files() {
        let registry = Registry::default();
        let id = crate::auth::random_token()[..32].to_owned();
        let path = std::env::temp_dir().join(format!("monitor-upload-test-{id}"));
        let mut upload = registry.begin(&id, "alice".into(), "theme", 2, 0, path.clone()).unwrap();
        std::fs::write(&path, b"hi").unwrap();
        upload.touched = Instant::now() - TTL;
        drop(upload);
        registry.cleanup();
        assert!(!path.exists());
    }
    #[tokio::test]
    async fn cancelling_a_request_keeps_the_upload_locked_until_worker_finishes() {
        let registry = Arc::new(Registry::default());
        let id = crate::auth::random_token()[..32].to_owned();
        let path = std::env::temp_dir().join(format!("monitor-cancel-{id}"));
        let upload = registry.begin(&id, "alice".into(), "theme", 8, 0, path.clone()).unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let request = tokio::spawn(crate::blocking::run(move || {
            ready.send(()).unwrap();
            blocked.recv().unwrap();
            drop(upload);
            let _ = done.send(());
        }));
        started.await.unwrap();
        request.abort();
        assert!(registry.begin(&id, "alice".into(), "theme", 8, 0, path.clone()).is_err());
        release.send(()).unwrap();
        finished.await.unwrap();
        assert!(registry.begin(&id, "alice".into(), "theme", 8, 0, path).is_ok());
    }
    #[test]
    fn crash_cleanup_removes_only_old_unowned_upload_files() {
        let directory = std::env::temp_dir().join(format!("monitor-orphans-{}", crate::auth::random_token()));
        std::fs::create_dir(&directory).unwrap();
        let name = |id: &str| directory.join(format!(".upload-{id}-0123456789abcdef.tar.gz"));
        let orphan = name("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let recent = name("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let active = name("cccccccccccccccccccccccccccccccc");
        let unrelated = directory.join("keep.tar.gz");
        let old = std::time::SystemTime::now() - TTL - Duration::from_secs(1);
        for path in [&orphan, &recent, &active, &unrelated] {
            std::fs::write(path, b"chunk").unwrap();
        }
        for path in [&orphan, &active, &unrelated] {
            std::fs::File::open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let registry = Registry::default();
        let upload = registry
            .begin("cccccccccccccccccccccccccccccccc", "alice".into(), "theme", 8, 0, active.clone())
            .unwrap();
        drop(upload);
        registry.cleanup_files(&directory, ".upload-", ".tar.gz");
        assert!(!orphan.exists());
        assert!(active.exists() && recent.exists() && unrelated.exists());
        drop(registry);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
