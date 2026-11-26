pub mod attachments;
pub mod config;
#[cfg(target_os = "android")]
mod android;
pub mod markdown;
pub mod notes;
pub mod sync;

slint::include_modules!();

use config::Config;
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
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
    /// Today's note didn't exist at launch: open (and create) it once the
    /// first sync is done, so a copy written on another device comes first.
    open_today_after_sync: Cell<bool>,
    /// What was shared to the app at launch, added once the first sync is done.
    pending_share: RefCell<Option<(String, Vec<SharedFile>)>>,
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

/// Why the app was started, when it wasn't from the launcher (Android).
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// Text and/or files sent from another app's share sheet: add them to
    /// today's note.
    Share { text: String, files: Vec<SharedFile> },
    /// The "Quick add" shortcut: show the quick-add bar.
    QuickAdd,
}

/// A file shared to the app, already copied to a staging path.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedFile {
    pub path: PathBuf,
    /// Its name as the sending app gave it.
    pub name: String,
}

/// Set before `run`.
static LAUNCH: Mutex<Option<Launch>> = Mutex::new(None);

pub fn set_launch(launch: Launch) {
    *LAUNCH.lock().unwrap() = Some(launch);
}

/// MdBlock.kind of an image shown in the preview (see ui/app.slint).
const PREVIEW_IMAGE: i32 = 11;

/// How long the "Added to …" confirmation shows before a share returns to
/// the app it came from.
const SHARE_DONE_DELAY: Duration = Duration::from_millis(1500);

/// Open a web page (or mail link, …) in the system's app for it.
fn platform_open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    return android::open_url(url);
    #[cfg(not(target_os = "android"))]
    return desktop_open(url.as_ref());
}

/// Open a note's attached file in the system's app for it. `note` is the
/// note it belongs to (Android names the exported copy after it).
fn platform_open_file(path: &std::path::Path, note: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    return android::open_file(path, note);
    #[cfg(not(target_os = "android"))]
    {
        let _ = note;
        desktop_open(path.as_os_str())
    }
}

#[cfg(not(target_os = "android"))]
fn desktop_open(what: &std::ffi::OsStr) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(what).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Desktop: files dragged from Finder (or any app) onto the window are
/// attached to the open note. Slint's DropArea doesn't receive drops from
/// other apps on this backend yet, so this listens to winit directly. winit
/// sends one event per file; files dropped together are handled as a batch.
#[cfg(not(target_os = "android"))]
fn watch_file_drops(ui: &AppWindow) {
    use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
    let batch: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    ui.window().on_winit_window_event(move |_, event| {
        match event {
            WindowEvent::HoveredFile(_) => with_app(|a| a.drag_hover()),
            WindowEvent::HoveredFileCancelled => with_app(|a| a.drag_cancelled()),
            WindowEvent::DroppedFile(path) => {
                let first = batch.borrow().is_empty();
                batch.borrow_mut().push(path.clone());
                if first {
                    let batch = batch.clone();
                    Timer::single_shot(Duration::ZERO, move || {
                        let paths = std::mem::take(&mut *batch.borrow_mut());
                        with_app(|a| a.drop_files(paths));
                    });
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

/// The platform's fixed-width font, used when Settings leaves it empty.
/// (Slint doesn't resolve the generic name "monospace" on every platform.)
fn default_mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "android") {
        "Droid Sans Mono"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

/// Set the editor font from the config.
fn apply_font(ui: &AppWindow, cfg: &Config) {
    let family = if cfg.editor_font.is_empty() { default_mono_font() } else { cfg.editor_font.as_str() };
    let font = ui.global::<EditorFont>();
    font.set_family(family.into());
    font.set_size(cfg.editor_font_size as f32);
}

/// Close the app's window on Android (back to the app a share came from).
/// Nothing to do on desktop, where shares don't happen.
fn finish_activity() {
    #[cfg(target_os = "android")]
    android::finish_current();
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
                self.set_text(&text);
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
        self.set_text("");
        ui.set_dirty(false);
        ui.set_preview(false);
        self.close_history();
    }

    /// Replace the editor's whole text (opening, restoring, reloading).
    fn set_text(&self, text: &str) {
        self.ui().set_editor_text(text.into());
    }

    /// The user changed the text: mark it dirty and schedule the autosave.
    fn mark_edited(self: &Rc<Self>) {
        self.ui().set_dirty(true);
        let weak = Rc::downgrade(self);
        self.save_timer.start(TimerMode::SingleShot, AUTOSAVE_DELAY, move || {
            if let Some(a) = weak.upgrade() {
                a.save_now();
                a.refresh_list();
            }
        });
    }


    /// Show the quick-add bar (hidden otherwise), prefilled with `text`.
    fn show_quick_add(&self, text: &str) {
        let ui = self.ui();
        ui.set_quick_text(text.into());
        ui.set_quick_visible(true);
    }

    /// Append `- HH:MM text` to today's note without opening it.
    fn quick_add(self: &Rc<Self>, text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let line = notes::quick_line(chrono::Local::now().time(), text);
        let added = self.add_to_today(|_, _| Ok(vec![line]));
        if added {
            self.ui().set_quick_text(SharedString::new());
            self.ui().set_quick_visible(false);
        }
        added
    }

    /// Append lines to today's note (creating it), refresh what's shown and
    /// queue a sync. `make` gets the notes dir and today's note path, and may
    /// first store attachments next to it.
    fn add_to_today(
        self: &Rc<Self>,
        make: impl FnOnce(&std::path::Path, &str) -> std::io::Result<Vec<String>>,
    ) -> bool {
        self.add_to_note(None, make)
    }

    /// Like `add_to_today`, for note `note` (today's note when `None`).
    fn add_to_note(
        self: &Rc<Self>,
        note: Option<String>,
        make: impl FnOnce(&std::path::Path, &str) -> std::io::Result<Vec<String>>,
    ) -> bool {
        self.save_now();
        let dir = &self.shared.notes_dir;
        let res = {
            let _g = self.shared.fs_lock.lock().unwrap();
            let rel = match note {
                Some(rel) => Ok(rel),
                None => notes::today(dir, chrono::Local::now().date_naive()),
            };
            rel.and_then(|rel| {
                let lines = make(dir, &rel)?;
                for line in &lines {
                    notes::append_line(dir, &rel, line)?;
                }
                Ok((rel, lines.len()))
            })
        };
        match res {
            Ok((rel, n)) => {
                // If that note is open, show the new lines there too.
                if self.editor.borrow().path.as_deref() == Some(rel.as_str()) {
                    if let Ok(t) = notes::read(dir, &rel) {
                        self.editor.borrow_mut().saved = t.clone();
                        self.set_text(&t);
                        if self.ui().get_preview() {
                            self.update_preview();
                        }
                    }
                }
                self.refresh_list();
                let what = if n == 1 { String::new() } else { format!("{n} items ") };
                self.show_message(format!("Added {what}to {rel}"), 4);
                self.engine.request();
                true
            }
            Err(e) => {
                self.show_message(format!("Could not add to the note: {e}"), 6);
                false
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    /// The note files dropped on the window go to: the open one, else today's.
    fn drop_target(&self) -> Option<String> {
        self.editor.borrow().path.clone()
    }

    #[cfg(not(target_os = "android"))]
    /// Files dragged over the window (desktop): say where they'd go.
    fn drag_hover(&self) {
        let target = self.drop_target().unwrap_or_else(|| "today's note".into());
        self.ui().set_message(format!("Drop to attach to {target}").into());
    }

    #[cfg(not(target_os = "android"))]
    fn drag_cancelled(&self) {
        self.ui().set_message(SharedString::new());
    }

    #[cfg(not(target_os = "android"))]
    /// Files dropped on the window (desktop): copy them into the note's
    /// attachment folder and link each one, as for files shared on Android.
    fn drop_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let (files, dirs): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| p.is_file());
        if files.is_empty() {
            if !dirs.is_empty() {
                self.show_message("Folders can't be attached; drop the files inside them", 6);
            }
            return;
        }
        let note = self.drop_target();
        let opened_today = note.is_none();
        let time = chrono::Local::now().time();
        let added = self.add_to_note(note, |dir, rel| {
            let mut lines = Vec::new();
            for f in &files {
                let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let target = attachments::add(dir, rel, f, &name, attachments::Take::Copy)?;
                let label = target.rsplit('/').next().unwrap_or(&target).to_string();
                lines.push(attachments::link_line(time, &label, &target));
            }
            Ok(lines)
        });
        if added && opened_today {
            self.open_today();
        }
        if !dirs.is_empty() {
            self.show_message(format!("Attached {} file(s); folders were skipped", files.len()), 6);
        }
    }

    /// Text shared from another app: append it after the first sync (so a
    /// today's note made on another device is pulled in first), then return
    /// to that app.
    fn receive_share(self: &Rc<Self>, text: String, files: Vec<SharedFile>) {
        if text.trim().is_empty() && files.is_empty() {
            finish_activity();
            return;
        }
        self.ui().set_message("Adding to today's note…".into());
        *self.pending_share.borrow_mut() = Some((text, files));
    }

    /// Store shared files in today's note's attachment folder, link each one
    /// (and add any shared text), then return to the sending app.
    fn finish_pending_share(self: &Rc<Self>) {
        let Some((text, files)) = self.pending_share.borrow_mut().take() else { return };
        let time = chrono::Local::now().time();
        let added = self.add_to_today(|dir, rel| {
            let mut lines = Vec::new();
            for f in &files {
                let target = attachments::add(dir, rel, &f.path, &f.name, attachments::Take::Move)?;
                let label = target.rsplit('/').next().unwrap_or(&target).to_string();
                lines.push(attachments::link_line(time, &label, &target));
            }
            // Text shared along with files is often just the file name again.
            if !text.trim().is_empty() && !files.iter().any(|f| text.trim() == f.name) {
                lines.push(notes::quick_line(time, &text));
            }
            Ok(lines)
        });
        if added {
            slint::Timer::single_shot(SHARE_DONE_DELAY, finish_activity);
        }
    }

    /// A link in the preview was tapped: open the web page, or the attached
    /// file with the system's app for it.
    fn open_link(self: &Rc<Self>, target: &str) {
        let res = if attachments::is_url(target) {
            platform_open_url(target)
        } else {
            let note = self.editor.borrow().path.clone().unwrap_or_default();
            match attachments::resolve_link(&self.shared.notes_dir, &note, target) {
                Some(path) if path.is_file() => platform_open_file(&path, &note),
                _ => Err(format!("{target} was not found")),
            }
        };
        if let Err(e) = res {
            self.show_message(format!("Could not open: {e}"), 6);
        }
    }

    /// Open today's note, creating it if needed.
    fn open_today(self: &Rc<Self>) {
        let res = {
            let _g = self.shared.fs_lock.lock().unwrap();
            notes::today(&self.shared.notes_dir, chrono::Local::now().date_naive())
        };
        match res {
            Ok(rel) => {
                self.refresh_list();
                self.open(&rel);
            }
            Err(e) => self.show_message(format!("Could not open today's note: {e}"), 6),
        }
    }

    /// At launch: open today's note now if it exists, else after the first sync.
    fn open_today_at_launch(self: &Rc<Self>) {
        let name = format!("{}.md", chrono::Local::now().format("%Y_%m_%d"));
        if self.shared.notes_dir.join(&name).exists() {
            self.open(&name);
        } else {
            self.open_today_after_sync.set(true);
        }
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
        let rows: Vec<DiffRow> = match sync::git::file_diff(&self.shared.notes_dir, id, &path) {
            Ok(lines) => lines
                .into_iter()
                .map(|l| {
                    use sync::git::DiffLine::*;
                    let (kind, text) = match l {
                        Context(t) => (0, t),
                        Added(t) => (1, t),
                        Removed(t) => (2, t),
                        Gap => (3, String::new()),
                    };
                    DiffRow { kind, text: text.into() }
                })
                .collect(),
            Err(e) => vec![DiffRow { kind: 3, text: format!("Could not compute changes: {}", e.message()).into() }],
        };
        ui.set_history_diff(ModelRc::new(VecModel::from(rows)));
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
        self.set_text(&text);
        self.mark_edited();
        self.show_message(format!("Restored version from {when}"), 6);
    }

    fn close_history(&self) {
        self.history.borrow_mut().clear();
        let ui = self.ui();
        ui.set_show_history(false);
        ui.set_versions(ModelRc::default());
        ui.set_history_selected(SharedString::new());
        ui.set_history_text(SharedString::new());
        ui.set_history_diff(ModelRc::default());
    }

    fn update_preview(&self) {
        let ui = self.ui();
        let note = self.editor.borrow().path.clone().unwrap_or_default();
        let blocks: Vec<MdBlock> = markdown::parse(&ui.get_editor_text())
            .into_iter()
            .map(|b| {
                let mut kind = b.kind.code();
                let mut image = slint::Image::default();
                if b.kind == markdown::Kind::Link && attachments::is_image(&b.target) {
                    // Show local images inline; anything else stays a link to open.
                    if let Some(img) = attachments::resolve_link(&self.shared.notes_dir, &note, &b.target)
                        .and_then(|p| slint::Image::load_from_path(&p).ok())
                    {
                        kind = PREVIEW_IMAGE;
                        image = img;
                    }
                }
                MdBlock { kind, text: b.text.into(), indent: b.indent, image, target: b.target.into() }
            })
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
        ui.set_cfg_editor_font(cfg.editor_font.clone().into());
        ui.set_cfg_editor_font_size(cfg.editor_font_size.to_string().into());
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
        cfg.editor_font = ui.get_cfg_editor_font().trim().to_string();
        cfg.editor_font_size = ui
            .get_cfg_editor_font_size()
            .trim()
            .parse()
            .map(|n: u32| n.clamp(*config::FONT_SIZES.start(), *config::FONT_SIZES.end()))
            .unwrap_or(config::DEFAULT_FONT_SIZE);
        apply_font(&ui, &cfg);
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
                    self.show_message("Notes folder changed: restart diari to use it.", 8);
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
        if self.pending_share.borrow().is_some() {
            let weak = Rc::downgrade(self);
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(a) = weak.upgrade() {
                    a.finish_pending_share();
                }
            });
        }
        if self.open_today_after_sync.replace(false) && self.editor.borrow().path.is_none() {
            // Run after this handler so the list reflects what sync pulled in.
            let weak = Rc::downgrade(self);
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(a) = weak.upgrade() {
                    a.open_today();
                }
            });
        }
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
                    self.set_text(&text);
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
    let keys = Keys::load_or_generate(&data_dir, &format!("diari@{}", cfg.device_name))?;

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
    ui.set_default_font(default_mono_font().into());
    apply_font(&ui, &shared.config.lock().unwrap());

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
        open_today_after_sync: Cell::new(false),
        pending_share: RefCell::default(),
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
    ui.on_open_today(|| with_app(|a| a.open_today()));
    ui.on_open_link(|t| with_app(|a| a.open_link(&t)));
    ui.on_quick_add(|t| {
        with_app(|a| {
            a.quick_add(&t);
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
    ui.on_text_edited(|_| with_app(|a| a.mark_edited()));
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
            match Keys::generate(&a.shared.data_dir, &format!("diari@{device}")) {
                Ok(k) => {
                    *a.shared.keys.lock().unwrap() = k;
                    a.load_settings_into_ui();
                }
                Err(e) => a.show_message(format!("Could not generate key: {e}"), 8),
            }
        })
    });

    app.refresh_list();
    match LAUNCH.lock().unwrap().take() {
        Some(Launch::Share { text, files }) => app.receive_share(text, files),
        Some(Launch::QuickAdd) => app.show_quick_add(""),
        None => app.open_today_at_launch(),
    }
    app.engine.request(); // initial sync

    #[cfg(not(target_os = "android"))]
    watch_file_drops(&ui);

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
        android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("diari"),
    );
    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/local/tmp/diari"));
    // Shared files are copied here first, then moved into the note's
    // attachment folder once the first sync is done.
    let staging = data_dir.join("incoming");
    let _ = std::fs::remove_dir_all(&staging);
    if let Some(launch) = android::launch_request(&app, &staging) {
        set_launch(launch);
    }
    android::set_app(&app);
    android::register_quick_add_shortcut(&app);

    let app_for_events = app.clone();
    slint::android::init_with_event_listener(app, move |event| match event {
        PollEvent::Main(MainEvent::Resume { .. }) => request_sync(),
        PollEvent::Main(MainEvent::Pause) | PollEvent::Main(MainEvent::SaveState { .. }) => flush(),
        // Leaving diari ends this activity: edits are already saved (Pause) and
        // a sync is queued; the next launch starts fresh with its own intent.
        PollEvent::Main(MainEvent::Stop) => {
            request_sync();
            // Opening a link or file from diari comes back to the note.
            if !android::take_opening_other_app() {
                android::finish(&app_for_events);
            }
        }
        _ => {}
    })
    .expect("init Slint Android backend");

    let notes = data_dir.join("notes");
    if let Err(e) = run(data_dir, move |_| notes, false) {
        log::error!("diari exited with error: {e}");
    }
}
