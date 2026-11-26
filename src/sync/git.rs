//! One sync cycle: commit local changes, fetch, integrate (fast-forward or
//! merge onto the remote tip with keep-both conflict handling), push.

use git2::build::CheckoutBuilder;
use git2::{
    Commit, Delta, DiffFindOptions, DiffOptions, Patch, ErrorCode, FetchOptions, IndexAddOption, IndexEntry, Oid, PushOptions,
    RemoteCallbacks, Repository, RepositoryInitOptions, Signature, Tree,
};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Mutex;

pub struct Params<'a> {
    pub dir: &'a Path,
    /// Empty = no remote: only commit locally.
    pub remote_url: &'a str,
    pub branch: &'a str,
    pub author_name: &'a str,
    pub author_email: &'a str,
    pub device: &'a str,
    /// Serialises local filesystem/index mutations with UI saves.
    pub fs_lock: &'a Mutex<()>,
    pub callbacks: &'a dyn Fn() -> RemoteCallbacks<'a>,
}

#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// Conflict copies created during this sync (relative paths).
    pub conflicts: Vec<String>,
    /// The working tree was updated from the remote.
    pub workdir_changed: bool,
    pub pushed: bool,
}

const MAX_ATTEMPTS: usize = 3;

pub fn sync(p: &Params) -> Result<Outcome, git2::Error> {
    let repo = ensure_repo(p)?;
    let mut out = Outcome::default();

    if p.remote_url.is_empty() {
        let _g = p.fs_lock.lock().unwrap();
        commit_all(&repo, p)?;
        return Ok(out);
    }

    for attempt in 1..=MAX_ATTEMPTS {
        fetch(&repo, p)?;
        let remote_oid = remote_tip(&repo, p.branch);
        {
            let _g = p.fs_lock.lock().unwrap();
            commit_all(&repo, p)?;
            integrate(&repo, p, remote_oid, &mut out)?;
        }
        let local_oid = local_tip(&repo, p.branch);
        if local_oid.is_none() || local_oid == remote_oid {
            return Ok(out); // nothing to push
        }
        match push(&repo, p) {
            Ok(()) => {
                out.pushed = true;
                return Ok(out);
            }
            Err(e) if attempt < MAX_ATTEMPTS && is_rejection(&e) => {
                log::info!("push rejected ({}), retrying", e.message());
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn is_rejection(e: &git2::Error) -> bool {
    let m = e.message().to_lowercase();
    e.code() == ErrorCode::NotFastForward
        || m.contains("fast-forward")
        || m.contains("fetch first")
        || m.contains("rejected")
        || m.contains("stale info")
}

fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn local_tip(repo: &Repository, branch: &str) -> Option<Oid> {
    repo.refname_to_id(&branch_ref(branch)).ok()
}

fn remote_tip(repo: &Repository, branch: &str) -> Option<Oid> {
    repo.refname_to_id(&format!("refs/remotes/origin/{branch}")).ok()
}

fn ensure_repo(p: &Params) -> Result<Repository, git2::Error> {
    let repo = match Repository::open(p.dir) {
        Ok(r) => r,
        Err(_) => {
            std::fs::create_dir_all(p.dir).map_err(|e| git2::Error::from_str(&e.to_string()))?;
            let mut o = RepositoryInitOptions::new();
            o.initial_head(p.branch);
            log::info!("initialising git repo in {}", p.dir.display());
            Repository::init_opts(p.dir, &o)?
        }
    };
    // Keep HEAD on the configured branch.
    let want = branch_ref(p.branch);
    let head_target = repo.find_reference("HEAD").ok().and_then(|h| h.symbolic_target().ok().flatten().map(str::to_string));
    if head_target.as_deref() != Some(want.as_str()) {
        repo.set_head(&want)?;
    }
    if !p.remote_url.is_empty() {
        match repo.find_remote("origin") {
            Ok(r) if r.url().ok() == Some(p.remote_url) => {}
            Ok(_) => repo.remote_set_url("origin", p.remote_url)?,
            Err(_) => {
                repo.remote("origin", p.remote_url)?;
            }
        }
    }
    Ok(repo)
}

fn signature(p: &Params) -> Result<Signature<'static>, git2::Error> {
    Signature::now(p.author_name, p.author_email)
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Files that differ between two trees, for commit messages.
#[derive(Debug, Default, PartialEq)]
struct Changes {
    added: Vec<String>,
    edited: Vec<String>,
    deleted: Vec<String>,
    renamed: Vec<(String, String)>,
}

impl Changes {
    fn len(&self) -> usize {
        self.added.len() + self.edited.len() + self.deleted.len() + self.renamed.len()
    }
}

fn summarize(repo: &Repository, old: Option<&Tree>, new: &Tree) -> Result<Changes, git2::Error> {
    let mut diff = repo.diff_tree_to_tree(old, Some(new), None)?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    let mut c = Changes::default();
    let path = |f: git2::DiffFile| f.path().map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    for d in diff.deltas() {
        match d.status() {
            Delta::Added | Delta::Copied => c.added.push(path(d.new_file())),
            Delta::Deleted => c.deleted.push(path(d.old_file())),
            Delta::Renamed => c.renamed.push((path(d.old_file()), path(d.new_file()))),
            Delta::Modified | Delta::Typechange => c.edited.push(path(d.new_file())),
            _ => {}
        }
    }
    Ok(c)
}

const SUBJECT_MAX: usize = 72;
const SUBJECT_MAX_ITEMS: usize = 3;

/// Subject names the changed files (or counts them); body lists every file,
/// then any `extra` lines, then device and time.
fn format_message(c: &Changes, prefix: Option<&str>, extra: &[String], device: &str, time: &str) -> String {
    let mut items: Vec<String> = Vec::new();
    items.extend(c.added.iter().map(|f| format!("add {f}")));
    items.extend(c.edited.iter().map(|f| format!("edit {f}")));
    items.extend(c.deleted.iter().map(|f| format!("delete {f}")));
    items.extend(c.renamed.iter().map(|(a, b)| format!("rename {a} -> {b}")));

    let summary = if items.is_empty() {
        None
    } else {
        let listed = items.join(", ");
        let fits = items.len() <= SUBJECT_MAX_ITEMS
            && listed.len() + prefix.map_or(0, |p| p.len() + 2) <= SUBJECT_MAX;
        Some(if fits {
            listed
        } else if c.len() == 1 {
            items[0].clone()
        } else {
            format!("update {} notes", c.len())
        })
    };
    let subject = match (prefix, summary) {
        (Some(p), Some(s)) => format!("{p}: {s}"),
        (Some(p), None) => p.to_string(),
        (None, Some(s)) => capitalize(&s),
        (None, None) => format!("Sync from {device}"),
    };

    let mut body: Vec<String> = Vec::new();
    body.extend(c.added.iter().map(|f| format!("Added: {f}")));
    body.extend(c.edited.iter().map(|f| format!("Edited: {f}")));
    body.extend(c.deleted.iter().map(|f| format!("Deleted: {f}")));
    body.extend(c.renamed.iter().map(|(a, b)| format!("Renamed: {a} -> {b}")));
    body.extend(extra.iter().cloned());

    let mut msg = subject;
    msg.push_str("\n\n");
    if !body.is_empty() {
        msg.push_str(&body.join("\n"));
        msg.push_str("\n\n");
    }
    msg.push_str(&format!("Device: {device}\nTime: {time}\n"));
    msg
}

fn capitalize(s: &str) -> String {
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().chain(ch).collect(),
        None => String::new(),
    }
}

/// One committed version of a note.
#[derive(Debug, Clone)]
pub struct Version {
    pub id: String,
    pub short: String,
    pub time: String,
    pub summary: String,
    pub author: String,
}

fn blob_at(commit: &Commit, rel: &str) -> Option<Oid> {
    commit.tree().ok()?.get_path(Path::new(rel)).ok().map(|e| e.id())
}

/// Commits that changed `rel`, newest first. Walks first parents from HEAD
/// and stops where the file first appears. Empty if there's no repo yet.
pub fn file_history(dir: &Path, rel: &str, limit: usize) -> Result<Vec<Version>, git2::Error> {
    let Ok(repo) = Repository::open(dir) else { return Ok(Vec::new()) };
    let Ok(mut commit) = repo.head().and_then(|h| h.peel_to_commit()) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    loop {
        let Some(blob) = blob_at(&commit, rel) else { break };
        let parent = commit.parent(0).ok();
        let parent_blob = parent.as_ref().and_then(|p| blob_at(p, rel));
        if parent_blob != Some(blob) {
            let when = chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let id = commit.id().to_string();
            out.push(Version {
                short: id[..7].to_string(),
                id,
                time: when,
                summary: commit.summary().ok().flatten().unwrap_or_default().to_string(),
                author: commit.author().name().unwrap_or_default().to_string(),
            });
            if out.len() >= limit {
                break;
            }
        }
        match parent {
            Some(p) if parent_blob.is_some() => commit = p,
            _ => break,
        }
    }
    Ok(out)
}

/// Contents of `rel` at `commit_id`, or `None` if it didn't exist there.
pub fn file_at(dir: &Path, commit_id: &str, rel: &str) -> Result<Option<String>, git2::Error> {
    let repo = Repository::open(dir)?;
    let commit = repo.find_commit(Oid::from_str(commit_id)?)?;
    let Some(id) = blob_at(&commit, rel) else { return Ok(None) };
    let blob = repo.find_blob(id)?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

/// One line of a version's changes.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Context(String),
    /// Separates hunks: lines skipped between them.
    Gap,
}

const DIFF_CONTEXT_LINES: u32 = 2;

/// What the version at `commit_id` changed in `rel`, compared with the version
/// in its first parent (or with an empty file if the note was new then).
pub fn file_diff(dir: &Path, commit_id: &str, rel: &str) -> Result<Vec<DiffLine>, git2::Error> {
    let repo = Repository::open(dir)?;
    let commit = repo.find_commit(Oid::from_str(commit_id)?)?;
    let content = |c: &Commit| -> Result<Vec<u8>, git2::Error> {
        match blob_at(c, rel) {
            Some(id) => Ok(repo.find_blob(id)?.content().to_vec()),
            None => Ok(Vec::new()),
        }
    };
    let new = content(&commit)?;
    let old = match commit.parent(0) {
        Ok(p) => content(&p)?,
        Err(_) => Vec::new(),
    };
    let mut opts = DiffOptions::new();
    opts.context_lines(DIFF_CONTEXT_LINES);
    let patch = Patch::from_buffers(&old, None, &new, None, Some(&mut opts))?;
    let mut out = Vec::new();
    for h in 0..patch.num_hunks() {
        if h > 0 {
            out.push(DiffLine::Gap);
        }
        for l in 0..patch.num_lines_in_hunk(h)? {
            let line = patch.line_in_hunk(h, l)?;
            let text = String::from_utf8_lossy(line.content()).trim_end_matches(['\n', '\r']).to_string();
            match line.origin() {
                '+' => out.push(DiffLine::Added(text)),
                '-' => out.push(DiffLine::Removed(text)),
                ' ' => out.push(DiffLine::Context(text)),
                _ => {} // "no newline at end of file" markers
            }
        }
    }
    Ok(out)
}

/// Stage everything (except hidden files) and commit if the tree changed.
fn commit_all(repo: &Repository, p: &Params) -> Result<bool, git2::Error> {
    let mut index = repo.index()?;
    let mut skip_hidden = |path: &Path, _: &[u8]| -> i32 {
        let hidden = path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.starts_with('.') && s != ".gitignore"
        });
        if hidden { 1 } else { 0 }
    };
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, Some(&mut skip_hidden))?;
    index.update_all(["*"].iter(), None)?;
    index.write()?;
    let tree_id = index.write_tree()?;
    let refname = branch_ref(p.branch);
    let head: Option<Commit> = repo.find_reference(&refname).ok().and_then(|r| r.peel_to_commit().ok());
    let tree = repo.find_tree(tree_id)?;
    match &head {
        Some(h) if h.tree_id() == tree_id => return Ok(false),
        None if tree.is_empty() => return Ok(false),
        _ => {}
    }
    let sig = signature(p)?;
    let parents: Vec<&Commit> = head.iter().collect();
    let old_tree = head.as_ref().map(|h| h.tree()).transpose()?;
    let changes = summarize(repo, old_tree.as_ref(), &tree)?;
    let msg = format_message(&changes, None, &[], p.device, &timestamp());
    repo.commit(Some(&refname), &sig, &sig, &msg, &tree, &parents)?;
    log::info!("committed: {}", msg.lines().next().unwrap_or_default());
    Ok(true)
}

fn fetch(repo: &Repository, p: &Params) -> Result<(), git2::Error> {
    let mut remote = repo.find_remote("origin")?;
    let mut fo = FetchOptions::new();
    fo.remote_callbacks((p.callbacks)());
    let spec = format!("+refs/heads/{0}:refs/remotes/origin/{0}", p.branch);
    remote.fetch(&[spec], Some(&mut fo), None)
}

/// Point the branch at `target`, updating the working tree. Must be called
/// while HEAD still points at the old commit, so files removed in `target`
/// are deleted from disk.
fn move_to(repo: &Repository, branch: &str, target: Oid) -> Result<(), git2::Error> {
    let commit = repo.find_commit(target)?;
    let mut co = CheckoutBuilder::new();
    co.force();
    repo.checkout_tree(commit.as_object(), Some(&mut co))?;
    repo.reference(&branch_ref(branch), target, true, "diari: sync")?;
    Ok(())
}

fn integrate(repo: &Repository, p: &Params, remote: Option<Oid>, out: &mut Outcome) -> Result<(), git2::Error> {
    let Some(remote) = remote else { return Ok(()) }; // remote branch doesn't exist yet
    let Some(local) = local_tip(repo, p.branch) else {
        log::info!("adopting remote branch");
        move_to(repo, p.branch, remote)?;
        out.workdir_changed = true;
        return Ok(());
    };
    if local == remote || repo.graph_descendant_of(local, remote)? {
        return Ok(()); // up to date or only local commits
    }
    if repo.graph_descendant_of(remote, local)? {
        log::info!("fast-forward to {remote}");
        move_to(repo, p.branch, remote)?;
        out.workdir_changed = true;
        return Ok(());
    }

    // Diverged: merge local changes onto the remote tip as one linear commit.
    log::info!("histories diverged; merging local changes onto {remote}");
    let local_c = repo.find_commit(local)?;
    let remote_c = repo.find_commit(remote)?;
    let mut idx = repo.merge_commits(&local_c, &remote_c, None)?;
    if idx.has_conflicts() {
        let conflicts: Vec<_> = idx.conflicts()?.collect::<Result<_, _>>()?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        for c in conflicts {
            let Some(any) = c.our.as_ref().or(c.their.as_ref()).or(c.ancestor.as_ref()) else { continue };
            let path_bytes = any.path.clone();
            let path = String::from_utf8_lossy(&path_bytes).into_owned();
            idx.conflict_remove(Path::new(&path))?;
            match (c.our, c.their) {
                // Local is just the empty template of a new note (e.g. today's
                // note made before this device ever synced): take the remote.
                (Some(ours), Some(theirs)) if is_blank_note(repo, &path, ours.id) => {
                    log::info!("{path}: local copy was an empty new note; keeping the remote one");
                    add_stage0(&mut idx, theirs, path_bytes)?;
                }
                (Some(ours), Some(theirs)) => {
                    // Remote wins the original name; local copy is kept aside.
                    let copy = conflict_name(&path, p.device, &stamp);
                    add_stage0(&mut idx, theirs, path_bytes)?;
                    add_stage0(&mut idx, ours, copy.clone().into_bytes())?;
                    log::warn!("conflict in {path}; local version saved as {copy}");
                    out.conflicts.push(copy);
                }
                // Modified on one side, deleted on the other: keep the content.
                (Some(e), None) | (None, Some(e)) => add_stage0(&mut idx, e, path_bytes)?,
                (None, None) => {}
            }
        }
    }
    let tree_id = idx.write_tree_to(repo)?;
    let tree = repo.find_tree(tree_id)?;
    let sig = signature(p)?;
    let changes = summarize(repo, Some(&remote_c.tree()?), &tree)?;
    let extra: Vec<String> = out.conflicts.iter().map(|c| format!("Conflict copy: {c}")).collect();
    let prefix = format!("Merge local changes from {}", p.device);
    let msg = format_message(&changes, Some(&prefix), &extra, p.device, &timestamp());
    let merged = repo.commit(None, &sig, &sig, &msg, &tree, &[&remote_c])?;
    move_to(repo, p.branch, merged)?;
    out.workdir_changed = true;
    Ok(())
}

fn add_stage0(idx: &mut git2::Index, mut e: IndexEntry, path: Vec<u8>) -> Result<(), git2::Error> {
    const STAGE_MASK: u16 = 0x3000;
    const NAME_MASK: u16 = 0x0fff;
    e.flags = (e.flags & !(STAGE_MASK | NAME_MASK)) | (path.len().min(NAME_MASK as usize) as u16);
    e.flags_extended = 0;
    e.path = path;
    idx.add(&e)
}

/// Is this blob a note with no content beyond its "# title" heading (what
/// `notes::create` writes)?
fn is_blank_note(repo: &Repository, path: &str, id: Oid) -> bool {
    let Ok(blob) = repo.find_blob(id) else { return false };
    let Ok(text) = std::str::from_utf8(blob.content()) else { return false };
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let text = text.trim();
    text.is_empty() || text == format!("# {stem}")
}

/// `notes/todo.md` -> `notes/todo.conflict-<device>-<stamp>.md`
pub fn conflict_name(path: &str, device: &str, stamp: &str) -> String {
    let device: String = device
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    format!("{dir}{stem}.conflict-{device}-{stamp}{ext}")
}

fn push(repo: &Repository, p: &Params) -> Result<(), git2::Error> {
    let mut remote = repo.find_remote("origin")?;
    let rejected: RefCell<Option<String>> = RefCell::new(None);
    {
        let mut cb = (p.callbacks)();
        cb.push_update_reference(|_refname, status| {
            if let Some(s) = status {
                *rejected.borrow_mut() = Some(s.to_string());
            }
            Ok(())
        });
        let mut po = PushOptions::new();
        po.remote_callbacks(cb);
        let spec = format!("refs/heads/{0}:refs/heads/{0}", p.branch);
        remote.push(&[spec], Some(&mut po))?;
    }
    if let Some(msg) = rejected.into_inner() {
        return Err(git2::Error::from_str(&format!("push rejected: {msg}")));
    }
    // Record what we pushed so the next sync sees the remote as up to date.
    if let Some(local) = local_tip(repo, p.branch) {
        repo.reference(&format!("refs/remotes/origin/{}", p.branch), local, true, "diari: push")?;
    }
    log::info!("pushed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct Dev {
        dir: PathBuf,
        name: &'static str,
    }

    fn sync_dev(d: &Dev, remote: &str) -> Outcome {
        let lock = Mutex::new(());
        let cb = || RemoteCallbacks::new();
        let p = Params {
            dir: &d.dir,
            remote_url: remote,
            branch: "main",
            author_name: "t",
            author_email: "t@t",
            device: d.name,
            fs_lock: &lock,
            callbacks: &cb,
        };
        sync(&p).expect("sync failed")
    }

    fn setup() -> (tempfile::TempDir, String, Dev, Dev) {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("remote.git");
        Repository::init_bare(&bare).unwrap();
        let url = format!("file://{}", bare.display());
        let a = Dev { dir: tmp.path().join("a"), name: "mac" };
        let b = Dev { dir: tmp.path().join("b"), name: "phone" };
        (tmp, url, a, b)
    }

    fn read(d: &Dev, f: &str) -> String {
        fs::read_to_string(d.dir.join(f)).unwrap()
    }

    #[test]
    fn local_only_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Dev { dir: tmp.path().join("n"), name: "x" };
        fs::create_dir_all(&d.dir).unwrap();
        fs::write(d.dir.join("a.md"), "a").unwrap();
        sync_dev(&d, "");
        let repo = Repository::open(&d.dir).unwrap();
        assert!(local_tip(&repo, "main").is_some());
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.summary().unwrap(), Some("Add a.md"));
        assert!(head.message().unwrap().contains("Added: a.md\n"));
    }

    #[test]
    fn attachments_are_committed() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Dev { dir: tmp.path().join("n"), name: "x" };
        fs::create_dir_all(d.dir.join("a.md.files")).unwrap();
        fs::write(d.dir.join("a.md"), "- [p.jpg](a.md.files/p.jpg)\n").unwrap();
        fs::write(d.dir.join("a.md.files/p.jpg"), [0xFFu8, 0xD8, 0xFF, 0x00]).unwrap();
        sync_dev(&d, "");
        let repo = Repository::open(&d.dir).unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        let blob = head.tree().unwrap().get_path(Path::new("a.md.files/p.jpg")).unwrap();
        assert_eq!(repo.find_blob(blob.id()).unwrap().content(), &[0xFF, 0xD8, 0xFF, 0x00]);
        assert!(head.message().unwrap().contains("Added: a.md.files/p.jpg\n"));
    }

    #[test]
    fn message_format() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let one = Changes { edited: s(&["2026_11_26.md"]), ..Default::default() };
        assert_eq!(
            format_message(&one, None, &[], "mac", "T"),
            "Edit 2026_11_26.md\n\nEdited: 2026_11_26.md\n\nDevice: mac\nTime: T\n"
        );

        let mix = Changes {
            added: s(&["todo.md"]),
            edited: s(&["a.md"]),
            deleted: s(&["old.md"]),
            renamed: vec![],
        };
        let m = format_message(&mix, None, &[], "mac", "T");
        assert!(m.starts_with("Add todo.md, edit a.md, delete old.md\n\nAdded: todo.md\nEdited: a.md\nDeleted: old.md\n"));

        let many = Changes { edited: s(&["a.md", "b.md", "c.md", "d.md"]), ..Default::default() };
        assert!(format_message(&many, None, &[], "mac", "T").starts_with("Update 4 notes\n"));

        let ren = Changes { renamed: vec![("a.md".into(), "w/a.md".into())], ..Default::default() };
        assert!(format_message(&ren, None, &[], "mac", "T").starts_with("Rename a.md -> w/a.md\n\nRenamed: a.md -> w/a.md\n"));

        let none = Changes::default();
        assert_eq!(format_message(&none, None, &[], "mac", "T"), "Sync from mac\n\nDevice: mac\nTime: T\n");

        let merged = format_message(&one, Some("Merge local changes from mac"), &s(&["Conflict copy: x.md"]), "mac", "T");
        assert!(merged.starts_with("Merge local changes from mac: edit 2026_11_26.md\n\nEdited: 2026_11_26.md\nConflict copy: x.md\n"));
    }

    #[test]
    fn history_and_file_at() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Dev { dir: tmp.path().join("n"), name: "x" };
        fs::create_dir_all(&d.dir).unwrap();
        assert!(file_history(&d.dir, "n.md", 10).unwrap().is_empty());
        for (i, other) in [(1, false), (2, true), (3, false)] {
            fs::write(d.dir.join("n.md"), format!("v{i}")).unwrap();
            sync_dev(&d, "");
            if other {
                fs::write(d.dir.join("other.md"), format!("o{i}")).unwrap();
                sync_dev(&d, "");
            }
        }
        let h = file_history(&d.dir, "n.md", 10).unwrap();
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].summary, "Edit n.md");
        assert_eq!(h[2].summary, "Add n.md");
        let texts: Vec<_> = h.iter().map(|v| file_at(&d.dir, &v.id, "n.md").unwrap().unwrap()).collect();
        assert_eq!(texts, vec!["v3", "v2", "v1"]);
        assert_eq!(file_at(&d.dir, &h[2].id, "other.md").unwrap(), None);
        assert!(file_history(&d.dir, "missing.md", 10).unwrap().is_empty());

        use DiffLine::*;
        assert_eq!(file_diff(&d.dir, &h[2].id, "n.md").unwrap(), vec![Added("v1".into())]);
        assert_eq!(
            file_diff(&d.dir, &h[0].id, "n.md").unwrap(),
            vec![Removed("v2".into()), Added("v3".into())]
        );
        assert_eq!(file_history(&d.dir, "n.md", 2).unwrap().len(), 2);
    }

    #[test]
    fn fast_forward_and_deletions() {
        let (_t, url, a, b) = setup();
        fs::create_dir_all(&a.dir).unwrap();
        fs::write(a.dir.join("one.md"), "hello").unwrap();
        fs::write(a.dir.join(".DS_Store"), "junk").unwrap();
        assert!(sync_dev(&a, &url).pushed);

        // Fresh device adopts the remote.
        let o = sync_dev(&b, &url);
        assert!(o.workdir_changed);
        assert_eq!(read(&b, "one.md"), "hello");
        assert!(!b.dir.join(".DS_Store").exists());

        // Edit + delete on b, fast-forward on a.
        fs::write(b.dir.join("one.md"), "hello 2").unwrap();
        fs::write(b.dir.join("two.md"), "two").unwrap();
        sync_dev(&b, &url);
        sync_dev(&a, &url);
        assert_eq!(read(&a, "one.md"), "hello 2");
        assert_eq!(read(&a, "two.md"), "two");

        fs::remove_file(a.dir.join("two.md")).unwrap();
        sync_dev(&a, &url);
        sync_dev(&b, &url);
        assert!(!b.dir.join("two.md").exists());
    }

    #[test]
    fn diverged_without_conflict() {
        let (_t, url, a, b) = setup();
        fs::create_dir_all(&a.dir).unwrap();
        fs::write(a.dir.join("base.md"), "line1\nline2\nline3\n").unwrap();
        sync_dev(&a, &url);
        sync_dev(&b, &url);

        fs::write(a.dir.join("a.md"), "from a").unwrap();
        fs::write(a.dir.join("base.md"), "LINE1\nline2\nline3\n").unwrap();
        fs::write(b.dir.join("b.md"), "from b").unwrap();
        fs::write(b.dir.join("base.md"), "line1\nline2\nLINE3\n").unwrap();
        sync_dev(&a, &url);
        let o = sync_dev(&b, &url);
        assert!(o.conflicts.is_empty());
        assert!(o.pushed);
        sync_dev(&a, &url);
        for d in [&a, &b] {
            assert_eq!(read(d, "a.md"), "from a");
            assert_eq!(read(d, "b.md"), "from b");
            assert_eq!(read(d, "base.md"), "LINE1\nline2\nLINE3\n");
        }
    }

    #[test]
    fn conflict_keeps_both() {
        let (_t, url, a, b) = setup();
        fs::create_dir_all(a.dir.join("sub")).unwrap();
        fs::write(a.dir.join("sub/n.md"), "base\n").unwrap();
        sync_dev(&a, &url);
        sync_dev(&b, &url);

        fs::write(a.dir.join("sub/n.md"), "from mac\n").unwrap();
        fs::write(b.dir.join("sub/n.md"), "from phone\n").unwrap();
        sync_dev(&a, &url);
        let o = sync_dev(&b, &url);
        assert_eq!(o.conflicts.len(), 1);
        let copy = &o.conflicts[0];
        assert!(copy.starts_with("sub/n.conflict-phone-") && copy.ends_with(".md"), "{copy}");
        assert_eq!(read(&b, "sub/n.md"), "from mac\n");
        assert_eq!(read(&b, copy), "from phone\n");
        let repo_b = Repository::open(&b.dir).unwrap();
        let head = repo_b.head().unwrap().peel_to_commit().unwrap();
        let msg = head.message().unwrap().to_string();
        assert!(msg.starts_with("Merge local changes from phone"), "{msg}");
        assert!(msg.contains(&format!("Conflict copy: {copy}\n")), "{msg}");

        sync_dev(&a, &url);
        assert_eq!(read(&a, "sub/n.md"), "from mac\n");
        assert_eq!(read(&a, copy), "from phone\n");
    }

    #[test]
    fn unrelated_histories_merge() {
        let (_t, url, a, b) = setup();
        fs::create_dir_all(&a.dir).unwrap();
        fs::write(a.dir.join("x.md"), "a").unwrap();
        sync_dev(&a, &url);
        // b already has notes before ever syncing.
        fs::create_dir_all(&b.dir).unwrap();
        fs::write(b.dir.join("y.md"), "b").unwrap();
        fs::write(b.dir.join("x.md"), "b's x").unwrap();
        let o = sync_dev(&b, &url);
        assert_eq!(o.conflicts.len(), 1);
        assert_eq!(read(&b, "x.md"), "a");
        assert_eq!(read(&b, &o.conflicts[0]), "b's x");
        assert_eq!(read(&b, "y.md"), "b");
    }

    #[test]
    fn blank_new_note_takes_remote_without_conflict() {
        let (_t, url, a, b) = setup();
        fs::create_dir_all(&a.dir).unwrap();
        fs::write(a.dir.join("2026_11_27.md"), "# 2026_11_27\n\nreal text\n").unwrap();
        sync_dev(&a, &url);
        // A new device made today's (empty) note before its first sync.
        fs::create_dir_all(&b.dir).unwrap();
        fs::write(b.dir.join("2026_11_27.md"), "# 2026_11_27\n\n").unwrap();
        sync_dev(&b, "");
        let o = sync_dev(&b, &url);
        assert!(o.conflicts.is_empty(), "{:?}", o.conflicts);
        assert_eq!(read(&b, "2026_11_27.md"), "# 2026_11_27\n\nreal text\n");
    }

    #[test]
    fn conflict_names() {
        assert_eq!(conflict_name("a.md", "my mac", "1"), "a.conflict-my_mac-1.md");
        assert_eq!(conflict_name("d/e/a.b.md", "x", "1"), "d/e/a.b.conflict-x-1.md");
        assert_eq!(conflict_name("README", "x", "1"), "README.conflict-x-1");
    }
}
