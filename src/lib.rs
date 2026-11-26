pub mod config;
pub mod markdown;
pub mod notes;
pub mod sync;

slint::include_modules!();

use config::Config;
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sync::auth::{KnownHosts, Keys};
use sync::{Report, Shared, SyncEngine};

const AUTOSAVE_DELAY: Duration = Duration::from_secs(1);
const SYNC_AFTER_SAVE_DELAY: Duration = Duration::from_secs(5);
const HISTORY_LIMIT: usize = 200;

/// Per-note editor state that isn't mirrored in the UI.
#[derive(Default)]
struct EditorState {
    path: Option<String>,
    /// Content last written to / read from disk.
    saved: String,
}

struct App {
    ui: slint::Weak<AppWindow>,
    shared: Arc<Shared>,
    engine: SyncEngine,
    editor: RefCell<EditorState>,
    /// Versions shown in the history view of the open note.
    history: RefCell<Vec<sync::git::Version>>,
    save_timer: Timer,
    sync_timer: Timer,
    message_timer: Timer,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = APP.with(|a| a.borrow().clone()) {
        f(&app);
    }
}

/// Ask for a sync from anywhere on the UI thread (e.g. Android resume).
pub fn request_sync() {
    with_app(|a| a.engine.request());
}

/// Flush pending edits to disk (e.g. Android pause).
pub fn flush() {
    with_app(|a| a.save_now());
}

impl App {
    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    fn show_message(self: &Rc<Self>, msg: impl Into<SharedString>, secs: u64) {
        self.ui().set_message(msg.into());
        let weak = self.ui.clone();
        self.message_timer.start(TimerMode::SingleShot, Duration::from_secs(secs), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_message(SharedString::new());
            }
        });
    }

    fn refresh_list(&self) {
        let ui = self.ui();
        let items: Vec<NoteItem> = notes::search(&self.shared.notes_dir, &ui.get_filter())
            .into_iter()
            .map(|hit| {
                let n = hit.note;
                let modified: chrono::DateTime<chrono::Local> = n.modified.into();
                let title = n
                    .rel_path
                    .strip_suffix(".md")
                    .unwrap_or(&n.rel_path)
                    .to_string();
                let subtitle = hit
                    .snippet
                    .unwrap_or_else(|| modified.format("%Y-%m-%d %H:%M").to_string());
                NoteItem { path: n.rel_path.into(), title: title.into(), subtitle: subtitle.into() }
            })
            .collect();
        ui.set_notes(ModelRc::new(VecModel::from(items)));
        ui.set_default_new_name(
            notes::default_new_name(&self.shared.notes_dir, chrono::Local::now().date_naive()).into(),
        );
    }

    fn save_now(&self) {
        self.save_timer.stop();
        let ui = self.ui();
        let mut ed = self.editor.borrow_mut();
        let Some(path) = ed.path.clone() else { return };
        let text = ui.get_editor_text().to_string();
        if text == ed.saved {
            ui.set_dirty(false);
            return;
        }
        let res = {
            let _g = self.shared.fs_lock.lock().unwrap();
            notes::write(&self.shared.notes_dir, &path, &text)
        };
        match res {
            Ok(()) => {
                ed.saved = text;
                ui.set_dirty(false);
                let engine = self.engine.clone();
                self.sync_timer.start(TimerMode::SingleShot, SYNC_AFTER_SAVE_DELAY, move || engine.request());
            }
            Err(e) => {
                log::error!("save {path}: {e}");
                ui.set_message(format!("Could not save {path}: {e}").into());
            }
        }
    }

    fn open(self: &Rc<Self>, path: &str) {
        self.save_now();
        let ui = self.ui();
        match notes::read(&self.shared.notes_dir, path) {
            Ok(text) => {
                *self.editor.borrow_mut() = EditorState { path: Some(path.to_string()), saved: text.clone() };
                ui.set_editor_text(text.into());
                ui.set_current_path(path.into());
                ui.set_dirty(false);
                ui.set_preview(false);
                self.close_history();
            }
            Err(e) => self.show_message(format!("Could not open {path}: {e}"), 6),
        }
    }

    fn close_editor(&self) {
        *self.editor.borrow_mut() = EditorState::default();
        let ui = self.ui();
        ui.set_current_path(SharedString::new());
        ui.set_editor_text(SharedString::new());
        ui.set_dirty(false);
        ui.set_preview(false);
        self.close_history();
    }

    fn open_history(self: &Rc<Self>) {
        self.save_now();
        let Some(path) = self.editor.borrow().path.clone() else { return };
        let versions = match sync::git::file_history(&self.shared.notes_dir, &path, HISTORY_LIMIT) {
            Ok(v) => v,
            Err(e) => {
                self.show_message(format!("Could not read history: {}", e.message()), 6);
                return;
            }
        };
        let Some(newest) = versions.first().map(|v| v.id.clone()) else {
            self.show_message("No history yet — sync hasn't committed this note", 6);
            return;
        };
        let items: Vec<VersionItem> = versions
            .iter()
            .map(|v| VersionItem {
                id: v.id.clone().into(),
                title: v.time.clone().into(),
                subtitle: format!("{} · {}", v.summary, v.short).into(),
            })
            .collect();
        *self.history.borrow_mut() = versions;
        let ui = self.ui();
        ui.set_versions(ModelRc::new(VecModel::from(items)));
        ui.set_show_history(true);
        self.select_version(&newest);
    }

    fn select_version(self: &Rc<Self>, id: &str) {
        let Some(path) = self.editor.borrow().path.clone() else { return };
        let ui = self.ui();
        ui.set_history_selected(id.into());
        let text = match sync::git::file_at(&self.shared.notes_dir, id, &path) {
            Ok(Some(t)) => t,
            Ok(None) => "(file did not exist in this version)".to_string(),
            Err(e) => format!("(could not read this version: {})", e.message()),
        };
        ui.set_history_text(text.into());
    }

    fn restore_version(self: &Rc<Self>) {
        let ui = self.ui();
        let id = ui.get_history_selected().to_string();
        let Some(path) = self.editor.borrow().path.clone() else { return };
        let Ok(Some(text)) = sync::git::file_at(&self.shared.notes_dir, &id, &path) else {
            self.show_message("Could not restore this version", 6);
            return;
        };
        let when = self.history.borrow().iter().find(|v| v.id == id).map(|v| v.time.clone()).unwrap_or_default();
        self.close_history();
        ui.set_editor_text(text.into());
        ui.invoke_text_edited(ui.get_editor_text());
        self.show_message(format!("Restored version from {when}"), 6);
    }

    fn close_history(&self) {
        self.history.borrow_mut().clear();
        let ui = self.ui();
        ui.set_show_history(false);
        ui.set_versions(ModelRc::default());
        ui.set_history_selected(SharedString::new());
        ui.set_history_text(SharedString::new());
    }

    fn update_preview(&self) {
        let ui = self.ui();
        let blocks: Vec<MdBlock> = markdown::parse(&ui.get_editor_text())
            .into_iter()
            .map(|b| MdBlock { kind: b.kind.code(), text: b.text.into(), indent: b.indent })
            .collect();
        ui.set_blocks(ModelRc::new(VecModel::from(blocks)));
    }

    fn load_settings_into_ui(&self) {
        let ui = self.ui();
        let cfg = self.shared.config.lock().unwrap().clone();
        ui.set_cfg_remote(cfg.remote_url.into());
        ui.set_cfg_branch(cfg.branch.into());
        ui.set_cfg_author_name(cfg.author_name.into());
        ui.set_cfg_author_email(cfg.author_email.into());
        ui.set_cfg_device(cfg.device_name.into());
        ui.set_cfg_notes_dir(self.shared.notes_dir.display().to_string().into());
        let keys = self.shared.keys.lock().unwrap();
        ui.set_public_key(keys.public_openssh.clone().into());
        ui.set_key_fingerprint(keys.fingerprint.clone().into());
    }

    fn save_settings(self: &Rc<Self>) {
        let ui = self.ui();
        let mut cfg = self.shared.config.lock().unwrap();
        cfg.remote_url = ui.get_cfg_remote().trim().to_string();
        cfg.branch = ui.get_cfg_branch().trim().to_string();
        if cfg.branch.is_empty() {
            cfg.branch = "main".into();
        }
        cfg.author_name = ui.get_cfg_author_name().trim().to_string();
        cfg.author_email = ui.get_cfg_author_email().trim().to_string();
        cfg.device_name = ui.get_cfg_device().trim().to_string();
        let mut restart_needed = false;
        if ui.get_desktop() {
            let dir = PathBuf::from(ui.get_cfg_notes_dir().trim());
            if !dir.as_os_str().is_empty() && dir != self.shared.notes_dir {
                cfg.notes_dir = Some(dir);
                restart_needed = true;
            }
        }
        let res = cfg.save(&self.shared.data_dir);
        drop(cfg);
        match res {
            Ok(()) => {
                ui.set_show_settings(false);
                if restart_needed {
                    self.show_message("Notes folder changed: restart perb to use it.", 8);
                }
                self.engine.request();
            }
            Err(e) => self.show_message(format!("Could not save settings: {e}"), 8),
        }
    }

    fn on_sync_started(&self) {
        let ui = self.ui();
        ui.set_sync_state(1);
        ui.set_sync_text("Syncing…".into());
    }

    fn on_sync_done(self: &Rc<Self>, report: Report) {
        let ui = self.ui();
        let now = chrono::Local::now().format("%H:%M").to_string();
        let local_only = self.shared.config.lock().unwrap().remote_url.trim().is_empty();
        match report {
            Ok(out) => {
                if !out.conflicts.is_empty() {
                    ui.set_sync_state(3);
                    ui.set_sync_text(format!("{} conflict(s) · {now}", out.conflicts.len()).into());
                    self.show_message(
                        format!("Conflicting edits were kept side by side: {}", out.conflicts.join(", ")),
                        15,
                    );
                } else if local_only {
                    ui.set_sync_state(4);
                    ui.set_sync_text("Local only".into());
                } else {
                    ui.set_sync_state(0);
                    ui.set_sync_text(format!("Synced {now}").into());
                }
                if out.workdir_changed {
                    self.reload_after_sync();
                }
            }
            Err(e) => {
                ui.set_sync_state(2);
                ui.set_sync_text("Sync error".into());
                self.show_message(format!("Sync failed: {e}"), 15);
            }
        }
    }

    /// Files may have changed on disk: refresh the list and the open note.
    fn reload_after_sync(self: &Rc<Self>) {
        self.refresh_list();
        let ui = self.ui();
        let path = self.editor.borrow().path.clone();
        let Some(path) = path else { return };
        if ui.get_dirty() {
            return; // unsaved edits win; they'll be committed next sync
        }
        match notes::read(&self.shared.notes_dir, &path) {
            Ok(text) => {
                if text != self.editor.borrow().saved {
                    self.editor.borrow_mut().saved = text.clone();
                    ui.set_editor_text(text.into());
                    if ui.get_preview() {
                        self.update_preview();
                    }
                }
            }
            Err(_) => {
                self.close_editor();
                self.show_message(format!("{path} was removed on another device"), 6);
            }
        }
    }
}

/// Run the app. `notes_dir` resolves the notes folder from the loaded config.
pub fn run(
    data_dir: PathBuf,
    notes_dir: impl FnOnce(&Config) -> PathBuf,
    desktop: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&data_dir)?;
    let cfg = Config::load(&data_dir);
    let notes_dir = notes_dir(&cfg);
    std::fs::create_dir_all(&notes_dir)?;
    log::info!("data dir {}, notes dir {}", data_dir.display(), notes_dir.display());
    let keys = Keys::load_or_generate(&data_dir, &format!("perb@{}", cfg.device_name))?;

    let shared = Arc::new(Shared {
        known_hosts: KnownHosts::load(&data_dir),
        data_dir,
        notes_dir,
        config: Mutex::new(cfg),
        keys: Mutex::new(keys),
        fs_lock: Mutex::new(()),
    });

    let ui = AppWindow::new()?;
    ui.set_desktop(desktop);

    let weak_start = ui.as_weak();
    let weak_done = ui.as_weak();
    let engine = SyncEngine::start(
        shared.clone(),
        move || {
            let _ = weak_start.upgrade_in_event_loop(|_| with_app(|a| a.on_sync_started()));
        },
        move |report| {
            let _ = weak_done.upgrade_in_event_loop(move |_| with_app(|a| a.on_sync_done(report)));
        },
    );

    let app = Rc::new(App {
        ui: ui.as_weak(),
        shared,
        engine,
        editor: RefCell::default(),
        history: RefCell::default(),
        save_timer: Timer::default(),
        sync_timer: Timer::default(),
        message_timer: Timer::default(),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    ui.on_open_note(|p| with_app(|a| a.open(&p)));
    ui.on_new_note(|name| {
        with_app(|a| {
            let res = {
                let _g = a.shared.fs_lock.lock().unwrap();
                notes::create(&a.shared.notes_dir, &name)
            };
            match res {
                Ok(rel) => {
                    a.refresh_list();
                    a.open(&rel);
                }
                Err(e) => a.show_message(format!("Could not create note: {e}"), 6),
            }
        })
    });
    ui.on_open_today(|| {
        with_app(|a| {
            let res = {
                let _g = a.shared.fs_lock.lock().unwrap();
                notes::today(&a.shared.notes_dir, chrono::Local::now().date_naive())
            };
            match res {
                Ok(rel) => {
                    a.refresh_list();
                    a.open(&rel);
                }
                Err(e) => a.show_message(format!("Could not open today's note: {e}"), 6),
            }
        })
    });
    ui.on_rename_note(|name| {
        with_app(|a| {
            a.save_now();
            let Some(old) = a.editor.borrow().path.clone() else { return };
            let res = {
                let _g = a.shared.fs_lock.lock().unwrap();
                notes::rename(&a.shared.notes_dir, &old, &name)
            };
            match res {
                Ok(new) => {
                    a.editor.borrow_mut().path = Some(new.clone());
                    a.ui().set_current_path(new.into());
                    a.close_history();
                    a.refresh_list();
                    a.engine.request();
                }
                Err(e) => {
                    a.ui().set_current_path(SharedString::new());
                    a.ui().set_current_path(old.into()); // reset the name field
                    a.show_message(format!("Could not rename: {e}"), 6);
                }
            }
        })
    });
    ui.on_delete_note(|| {
        with_app(|a| {
            let Some(path) = a.editor.borrow().path.clone() else { return };
            a.save_timer.stop();
            let res = {
                let _g = a.shared.fs_lock.lock().unwrap();
                notes::delete(&a.shared.notes_dir, &path)
            };
            match res {
                Ok(()) => {
                    a.close_editor();
                    a.refresh_list();
                    a.engine.request();
                }
                Err(e) => a.show_message(format!("Could not delete: {e}"), 6),
            }
        })
    });
    ui.on_text_edited(|_| {
        with_app(|a| {
            a.ui().set_dirty(true);
            let weak = Rc::downgrade(a);
            a.save_timer.start(TimerMode::SingleShot, AUTOSAVE_DELAY, move || {
                if let Some(a) = weak.upgrade() {
                    a.save_now();
                    a.refresh_list();
                }
            });
        })
    });
    ui.on_preview_toggled(|| {
        with_app(|a| {
            let ui = a.ui();
            let on = !ui.get_preview();
            if on {
                a.save_now();
                a.update_preview();
            }
            ui.set_preview(on);
        })
    });
    ui.on_open_history(|| with_app(|a| a.open_history()));
    ui.on_select_version(|id| with_app(|a| a.select_version(&id)));
    ui.on_restore_version(|| with_app(|a| a.restore_version()));
    ui.on_close_history(|| with_app(|a| a.close_history()));
    ui.on_back(|| {
        with_app(|a| {
            a.save_now();
            a.close_editor();
            a.refresh_list();
        })
    });
    ui.on_sync_now(|| {
        with_app(|a| {
            a.save_now();
            a.engine.request();
        })
    });
    ui.on_filter_edited(|_| with_app(|a| a.refresh_list()));
    ui.on_open_settings(|| {
        with_app(|a| {
            a.load_settings_into_ui();
            a.ui().set_show_settings(true);
        })
    });
    ui.on_save_settings(|| with_app(|a| a.save_settings()));
    ui.on_regenerate_key(|| {
        with_app(|a| {
            let device = a.shared.config.lock().unwrap().device_name.clone();
            match Keys::generate(&a.shared.data_dir, &format!("perb@{device}")) {
                Ok(k) => {
                    *a.shared.keys.lock().unwrap() = k;
                    a.load_settings_into_ui();
                }
                Err(e) => a.show_message(format!("Could not generate key: {e}"), 8),
            }
        })
    });

    app.refresh_list();
    app.engine.request(); // initial sync

    ui.run()?;

    app.save_now();
    APP.with(|a| *a.borrow_mut() = None);
    Ok(())
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    use slint::android::android_activity::{MainEvent, PollEvent};

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("perb"),
    );
    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/perb"));
    slint::android::init_with_event_listener(app, |event| match event {
        PollEvent::Main(MainEvent::Resume { .. }) => request_sync(),
        PollEvent::Main(MainEvent::Pause) | PollEvent::Main(MainEvent::SaveState { .. }) => flush(),
        _ => {}
    })
    .expect("init Slint Android backend");

    let notes = data_dir.join("notes");
    if let Err(e) = run(data_dir, move |_| notes, false) {
        log::error!("perb exited with error: {e}");
    }
}
