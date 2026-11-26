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
                // A note's attachments, not notes.
                if crate::attachments::is_attachment_dir(&name.to_string_lossy()) {
                    continue;
                }
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

/// A note that matches a search, with the first matching line of its text
/// (`None` when only the file name matched).
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub note: NoteInfo,
    pub snippet: Option<String>,
}

const SNIPPET_CHARS: usize = 80;

/// Notes whose path or text contains `query` (case-insensitive), most
/// recently modified first. An empty query returns every note.
pub fn search(dir: &Path, query: &str) -> Vec<SearchHit> {
    let q = query.trim().to_lowercase();
    list(dir)
        .into_iter()
        .filter_map(|note| {
            if q.is_empty() || note.rel_path.to_lowercase().contains(&q) {
                return Some(SearchHit { note, snippet: None });
            }
            let text = read(dir, &note.rel_path).ok()?;
            let line = text.lines().find(|l| l.to_lowercase().contains(&q))?;
            Some(SearchHit { snippet: Some(snippet(line, &q)), note })
        })
        .collect()
}

/// The matching line, trimmed and cut to about `SNIPPET_CHARS` characters
/// around the match, with `…` where text was cut.
fn snippet(line: &str, q: &str) -> String {
    let chars: Vec<char> = line.trim().chars().collect();
    if chars.len() <= SNIPPET_CHARS {
        return chars.into_iter().collect();
    }
    // Find the match position in characters, comparing case-insensitively.
    let qlen = q.chars().count();
    let at = (0..chars.len())
        .find(|&i| {
            let end = (i + qlen).min(chars.len());
            chars[i..end].iter().collect::<String>().to_lowercase().starts_with(q)
        })
        .unwrap_or(0);
    let start = at.saturating_sub(SNIPPET_CHARS / 3).min(chars.len() - SNIPPET_CHARS);
    let end = start + SNIPPET_CHARS;
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    s
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

/// Delete a note and its attachment folder.
pub fn delete(dir: &Path, rel: &str) -> io::Result<()> {
    std::fs::remove_file(resolve(dir, rel)?)?;
    let files = resolve(dir, &crate::attachments::dir_for(rel))?;
    if files.is_dir() {
        std::fs::remove_dir_all(files)?;
    }
    Ok(())
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
    std::fs::rename(from, &to)?;

    // Move the attachment folder along and point the note's links at it.
    let old_files = resolve(dir, &crate::attachments::dir_for(rel))?;
    if old_files.is_dir() {
        let new_files = resolve(dir, &crate::attachments::dir_for(&new_rel))?;
        std::fs::rename(&old_files, &new_files)?;
        let file_name = |r: &str| r.rsplit('/').next().unwrap_or(r).to_string();
        let (old_name, new_name) = (file_name(rel), file_name(&new_rel));
        let text = std::fs::read_to_string(&to)?;
        let fixed = text
            .replace(&format!("({old_name}.files/"), &format!("({new_name}.files/"))
            .replace(&format!("(<{old_name}.files/"), &format!("(<{new_name}.files/"));
        if fixed != text {
            atomic_write(&to, fixed.as_bytes())?;
        }
    }
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

/// Path of today's note (`yyyy_mm_dd.md`), creating it if it doesn't exist.
pub fn today(dir: &Path, today: chrono::NaiveDate) -> io::Result<String> {
    let name = format!("{}.md", today.format("%Y_%m_%d"));
    if resolve(dir, &name)?.exists() {
        Ok(name)
    } else {
        create(dir, &name)
    }
}

/// Append `line` to today's note (creating it if needed). Returns its path.
pub fn append_to_today(dir: &Path, day: chrono::NaiveDate, line: &str) -> io::Result<String> {
    let rel = today(dir, day)?;
    let mut text = read(dir, &rel)?;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    write(dir, &rel, &text)?;
    Ok(rel)
}

/// A quick-add entry: `- HH:MM text`, on one line.
pub fn quick_line(time: chrono::NaiveTime, text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- {} {text}", time.format("%H:%M"))
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
    fn today_opens_or_creates() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        assert_eq!(today(d, day).unwrap(), "2026_03_07.md");
        write(d, "2026_03_07.md", "kept").unwrap();
        assert_eq!(today(d, day).unwrap(), "2026_03_07.md");
        assert_eq!(read(d, "2026_03_07.md").unwrap(), "kept");
        assert_eq!(list(d).len(), 1);
    }

    #[test]
    fn search_names_and_text() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "2026_11_26.md", "# Day\n\n- Went to the Conference on AI\n").unwrap();
        write(d, "ideas.md", "nothing here\n").unwrap();
        write(d, "sub/conference notes.md", "x\n").unwrap();

        assert_eq!(search(d, "").len(), 3);

        let mut hits: Vec<_> = search(d, "CONFERENCE")
            .into_iter()
            .map(|h| (h.note.rel_path, h.snippet))
            .collect();
        hits.sort();
        assert_eq!(
            hits,
            vec![
                ("2026_11_26.md".to_string(), Some("- Went to the Conference on AI".to_string())),
                ("sub/conference notes.md".to_string(), None),
            ]
        );
        assert!(search(d, "missing").is_empty());
    }

    #[test]
    fn snippet_cuts_around_match() {
        let long = format!("{} needle {}", "a".repeat(100), "b".repeat(100));
        let s = snippet(&long, "needle");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("needle"));
        assert_eq!(s.chars().count(), SNIPPET_CHARS + 2);
        assert_eq!(snippet("  short line  ", "short"), "short line");
        let accents = format!("{}Café{}", "é".repeat(90), "x".repeat(90));
        assert!(snippet(&accents, "café").contains("Café"));
    }

    #[test]
    fn quick_add_appends_to_today() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let day = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap();
        let t = chrono::NaiveTime::from_hms_opt(9, 5, 0).unwrap();
        assert_eq!(quick_line(t, "  read\nthis   link "), "- 09:05 read this link");
        assert_eq!(append_to_today(d, day, "- 09:05 a").unwrap(), "2026_03_07.md");
        assert_eq!(read(d, "2026_03_07.md").unwrap(), "# 2026_03_07\n\n- 09:05 a\n");
        write(d, "2026_03_07.md", "no newline").unwrap();
        append_to_today(d, day, "- 09:06 b").unwrap();
        assert_eq!(read(d, "2026_03_07.md").unwrap(), "no newline\n- 09:06 b\n");
    }

    #[test]
    fn attachments_follow_their_note() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        write(d, "a.md", "- [p](a.md.files/p.jpg)\n").unwrap();
        std::fs::create_dir(d.join("a.md.files")).unwrap();
        std::fs::write(d.join("a.md.files/p.jpg"), b"x").unwrap();
        std::fs::write(d.join("a.md.files/notes.md"), b"not a note").unwrap();
        assert_eq!(list(d).len(), 1, "attachment folders are not listed");

        assert_eq!(rename(d, "a.md", "b").unwrap(), "b.md");
        assert!(d.join("b.md.files/p.jpg").exists() && !d.join("a.md.files").exists());
        assert_eq!(read(d, "b.md").unwrap(), "- [p](b.md.files/p.jpg)\n");

        delete(d, "b.md").unwrap();
        assert!(!d.join("b.md.files").exists());
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
