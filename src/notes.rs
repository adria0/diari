//! Markdown files in the notes folder. Paths handed to/from the UI are
//! relative to the notes dir and always use `/` separators.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct NoteInfo {
    pub rel_path: String,
    pub modified: SystemTime,
}

fn is_md(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// All markdown files, most recently modified first. Hidden entries
/// (including `.git`) are skipped.
pub fn list(dir: &Path) -> Vec<NoteInfo> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<NoteInfo>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk(root, &path, out);
            } else if ft.is_file() && is_md(&path) {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let rel = path.strip_prefix(root).unwrap_or(&path);
                out.push(NoteInfo { rel_path: to_rel_string(rel), modified });
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.rel_path.cmp(&b.rel_path)));
    out
}

fn to_rel_string(p: &Path) -> String {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve a UI-provided relative path, refusing anything that escapes `dir`.
pub fn resolve(dir: &Path, rel: &str) -> io::Result<PathBuf> {
    let mut out = dir.to_path_buf();
    let mut any = false;
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(io::Error::new(io::ErrorKind::InvalidInput, "path escapes notes dir")),
            p if p.starts_with('.') => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "hidden paths not allowed"))
            }
            p => {
                out.push(p);
                any = true;
            }
        }
    }
    if !any {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty path"));
    }
    Ok(out)
}

pub fn read(dir: &Path, rel: &str) -> io::Result<String> {
    let bytes = std::fs::read(resolve(dir, rel)?)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn write(dir: &Path, rel: &str, content: &str) -> io::Result<()> {
    atomic_write(&resolve(dir, rel)?, content.as_bytes())
}

pub fn delete(dir: &Path, rel: &str) -> io::Result<()> {
    std::fs::remove_file(resolve(dir, rel)?)
}

/// Rename a note; returns the new relative path.
pub fn rename(dir: &Path, rel: &str, new_name: &str) -> io::Result<String> {
    let from = resolve(dir, rel)?;
    let new_rel = normalize_name(new_name)?;
    let to = resolve(dir, &new_rel)?;
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a note with that name exists"));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)?;
    Ok(new_rel)
}

/// Turn a user-typed name into a relative `.md` path.
fn normalize_name(name: &str) -> io::Result<String> {
    let cleaned: Vec<&str> = name
        .trim()
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if cleaned.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty name"));
    }
    let mut rel = cleaned.join("/");
    if !is_md(Path::new(&rel)) {
        rel.push_str(".md");
    }
    Ok(rel)
}

/// Create a new note from a user-typed name (subfolders via `/` allowed).
/// If the name exists, a numeric suffix is added. Returns the relative path.
pub fn create(dir: &Path, name: &str) -> io::Result<String> {
    let rel = normalize_name(name)?;
    let base = rel.trim_end_matches(".md").to_string();
    let mut candidate = rel.clone();
    let mut n = 2;
    while resolve(dir, &candidate)?.exists() {
        candidate = format!("{base} {n}.md");
        n += 1;
    }
    let path = resolve(dir, &candidate)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let title = base.rsplit('/').next().unwrap_or(&base);
    atomic_write(&path, format!("# {title}\n\n").as_bytes())?;
    Ok(candidate)
}

/// Default name for a new note: today's date as `yyyy_mm_dd`, or empty if
/// `yyyy_mm_dd.md` already exists in the notes folder.
pub fn default_new_name(dir: &Path, today: chrono::NaiveDate) -> String {
    let name = today.format("%Y_%m_%d").to_string();
    if dir.join(format!("{name}.md")).exists() {
        String::new()
    } else {
        name
    }
}

/// Write via temp file + rename so a crash never leaves a half-written note.
pub fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent dir"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_write_rename_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let a = create(d, "Hello").unwrap();
        assert_eq!(a, "Hello.md");
        let b = create(d, "Hello").unwrap();
        assert_eq!(b, "Hello 2.md");
        let c = create(d, "sub/Deep.md").unwrap();
        assert_eq!(c, "sub/Deep.md");
        assert_eq!(read(d, &a).unwrap(), "# Hello\n\n");

        std::fs::create_dir(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/x.md"), "hidden").unwrap();
        std::fs::write(d.join("notes.txt"), "not md").unwrap();
        let mut names: Vec<_> = list(d).into_iter().map(|n| n.rel_path).collect();
        names.sort();
        assert_eq!(names, vec!["Hello 2.md", "Hello.md", "sub/Deep.md"]);

        write(d, &a, "changed").unwrap();
        assert_eq!(read(d, &a).unwrap(), "changed");
        let r = rename(d, &a, "Renamed").unwrap();
        assert_eq!(r, "Renamed.md");
        assert_eq!(read(d, &r).unwrap(), "changed");
        assert!(rename(d, &r, "Hello 2").is_err());
        delete(d, &r).unwrap();
        assert_eq!(list(d).len(), 2);
    }

    #[test]
    fn default_new_name_is_today_unless_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        assert_eq!(default_new_name(d, day), "2026_03_07");
        create(d, &default_new_name(d, day)).unwrap();
        assert_eq!(default_new_name(d, day), "");
    }

    #[test]
    fn resolve_rejects_escape() {
        let d = Path::new("/notes");
        assert!(resolve(d, "../etc/passwd").is_err());
        assert!(resolve(d, ".git/config").is_err());
        assert!(resolve(d, "").is_err());
        assert_eq!(resolve(d, "a/b.md").unwrap(), Path::new("/notes/a/b.md"));
    }
}
