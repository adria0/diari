//! One sync cycle: commit local changes, fetch, integrate (fast-forward or
//! merge onto the remote tip with keep-both conflict handling), push.

use git2::build::CheckoutBuilder;
use git2::{
    Commit, ErrorCode, FetchOptions, IndexAddOption, IndexEntry, Oid, PushOptions, RemoteCallbacks,
    Repository, RepositoryInitOptions, Signature,
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
        commit_all(&repo, p, "")?;
        return Ok(out);
    }

    for attempt in 1..=MAX_ATTEMPTS {
        fetch(&repo, p)?;
        let remote_oid = remote_tip(&repo, p.branch);
        {
            let _g = p.fs_lock.lock().unwrap();
            commit_all(&repo, p, "")?;
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

/// Stage everything (except hidden files) and commit if the tree changed.
fn commit_all(repo: &Repository, p: &Params, _why: &str) -> Result<bool, git2::Error> {
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
    let msg = format!("perb: {} {}", p.device, timestamp());
    repo.commit(Some(&refname), &sig, &sig, &msg, &tree, &parents)?;
    log::info!("committed local changes");
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
    repo.reference(&branch_ref(branch), target, true, "perb: sync")?;
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
    let msg = format!("perb: merge from {} {}", p.device, timestamp());
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
        repo.reference(&format!("refs/remotes/origin/{}", p.branch), local, true, "perb: push")?;
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
    fn conflict_names() {
        assert_eq!(conflict_name("a.md", "my mac", "1"), "a.conflict-my_mac-1.md");
        assert_eq!(conflict_name("d/e/a.b.md", "x", "1"), "d/e/a.b.conflict-x-1.md");
        assert_eq!(conflict_name("README", "x", "1"), "README.conflict-x-1");
    }
}
