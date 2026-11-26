//! Background sync engine. All git work happens on a worker thread; results are
//! reported through callbacks (which the UI forwards to the event loop).

pub mod auth;
pub mod git;

use crate::config::Config;
use auth::{KnownHosts, Keys};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use git::Outcome;

/// State shared between the UI thread and the sync worker.
pub struct Shared {
    pub data_dir: PathBuf,
    pub notes_dir: PathBuf,
    pub config: Mutex<Config>,
    pub keys: Mutex<Keys>,
    pub known_hosts: KnownHosts,
    /// Held while the UI writes files and while sync touches the work tree.
    pub fs_lock: Mutex<()>,
}

pub type Report = Result<Outcome, String>;

#[derive(Clone)]
pub struct SyncEngine {
    tx: Sender<()>,
}

impl SyncEngine {
    pub fn start(
        shared: Arc<Shared>,
        on_start: impl Fn() + Send + 'static,
        on_done: impl Fn(Report) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("diari-sync".into())
            .spawn(move || loop {
                let interval = shared.config.lock().unwrap().sync_interval_secs.max(30);
                match rx.recv_timeout(Duration::from_secs(interval)) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                while rx.try_recv().is_ok() {} // coalesce queued requests
                on_start();
                let r = run_once(&shared);
                if let Err(e) = &r {
                    log::error!("sync failed: {e}");
                }
                on_done(r);
            })
            .expect("spawn sync thread");
        Self { tx }
    }

    pub fn request(&self) {
        let _ = self.tx.send(());
    }
}

fn run_once(shared: &Shared) -> Report {
    let cfg = shared.config.lock().unwrap().clone();
    let keys = shared.keys.lock().unwrap().clone();
    let cb = || auth::callbacks(&keys, &shared.known_hosts);
    let params = git::Params {
        dir: &shared.notes_dir,
        remote_url: cfg.remote_url.trim(),
        branch: if cfg.branch.trim().is_empty() { "main" } else { cfg.branch.trim() },
        author_name: &cfg.author_name,
        author_email: &cfg.author_email,
        device: &cfg.device_name,
        fs_lock: &shared.fs_lock,
        callbacks: &cb,
    };
    git::sync(&params).map_err(|e| e.message().to_string())
}
