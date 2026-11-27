//! Files attached to a note. They live in a folder next to it named after the
//! note file plus `.files` (`2026_11_26.md` → `2026_11_26.md.files/`), and the
//! note links to them with ordinary markdown links, relative to the note.

use crate::notes;
use std::io;
use std::path::{Path, PathBuf};

const SUFFIX: &str = ".files";

/// Is this a note's attachment folder (hidden from the note list)?
pub fn is_attachment_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md.files") || lower.ends_with(".markdown.files")
}

/// Attachment folder of a note, relative to the notes dir.
pub fn dir_for(note_rel: &str) -> String {
    format!("{note_rel}{SUFFIX}")
}

/// A file name that is safe on every platform and in a markdown link:
/// letters, digits, `.`, `-` and `_`; anything else becomes `_`.
pub fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out: String = base
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    out = out.trim_matches('.').to_string();
    if out.is_empty() {
        out = "file".into();
    }
    out
}

/// How `add` takes the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// Move it (a staged copy of a shared file).
    Move,
    /// Copy it, leaving the original (a file dropped from Finder).
    Copy,
}

/// Put `src` into the note's attachment folder as `name`, adding `-2`, `-3`…
/// if taken. Returns the link target, relative to the note:
/// `2026_11_26.md.files/photo.jpg`.
pub fn add(notes_dir: &Path, note_rel: &str, src: &Path, name: &str, take: Take) -> io::Result<String> {
    let folder_rel = dir_for(note_rel);
    let folder = notes::resolve(notes_dir, &folder_rel)?;
    std::fs::create_dir_all(&folder)?;
    let name = sanitize(name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let mut candidate = name.clone();
    let mut n = 2;
    while folder.join(&candidate).exists() {
        candidate = format!("{stem}-{n}{ext}");
        n += 1;
    }
    let dest = folder.join(&candidate);
    match take {
        Take::Copy => {
            std::fs::copy(src, &dest)?;
        }
        Take::Move => {
            if std::fs::rename(src, &dest).is_err() {
                std::fs::copy(src, &dest)?;
                let _ = std::fs::remove_file(src);
            }
        }
    }
    let folder_name = folder_rel.rsplit('/').next().unwrap_or(&folder_rel);
    Ok(format!("{folder_name}/{candidate}"))
}

/// A file in a note's attachment folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub size: u64,
    pub path: PathBuf,
}

/// The files attached to a note, by name. Empty if it has none.
pub fn list(notes_dir: &Path, note_rel: &str) -> Vec<Attachment> {
    let Ok(folder) = notes::resolve(notes_dir, &dir_for(note_rel)) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&folder) else { return Vec::new() };
    let mut out: Vec<Attachment> = rd
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            (meta.is_file() && !name.starts_with('.')).then(|| Attachment { name, size: meta.len(), path: e.path() })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Delete one attached file (and the folder, if it was the last), and drop
/// the note's lines that only link to it (`- HH:MM [name](target)`). Returns
/// the note's new text if it changed.
pub fn delete(notes_dir: &Path, note_rel: &str, name: &str) -> io::Result<Option<String>> {
    if name.contains('/') || name.starts_with('.') || name.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad file name"));
    }
    let folder_rel = dir_for(note_rel);
    let folder = notes::resolve(notes_dir, &folder_rel)?;
    std::fs::remove_file(folder.join(name))?;
    if std::fs::read_dir(&folder).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = std::fs::remove_dir(&folder);
    }
    let folder_name = folder_rel.rsplit('/').next().unwrap_or(&folder_rel);
    let target = format!("{folder_name}/{name}");
    let text = notes::read(notes_dir, note_rel)?;
    let kept = remove_link_lines(&text, &target);
    if kept == text {
        return Ok(None);
    }
    notes::write(notes_dir, note_rel, &kept)?;
    Ok(Some(kept))
}

/// `text` without the lines that are only a link to `target`, optionally as
/// a list item with a time: `- 09:01 [photo.jpg](n.md.files/photo.jpg)`.
pub fn remove_link_lines(text: &str, target: &str) -> String {
    let is_link_line = |line: &str| {
        let mut l = line.trim();
        l = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(l).trim_start();
        let b = l.as_bytes();
        if b.len() > 6 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b':' && b[3].is_ascii_digit() && b[4].is_ascii_digit() && b[5] == b' ' {
            l = &l[6..];
        }
        // What's left must be exactly one link, `[label](target)` or `![..](..)`.
        let l = l.strip_prefix('!').unwrap_or(l);
        l.starts_with('[')
            && l.matches("](").count() == 1
            && (l.ends_with(&format!("]({target})")) || l.ends_with(&format!("](<{target}>)")))
    };
    let mut out: Vec<&str> = text.split('\n').filter(|l| !is_link_line(l)).collect();
    // Don't leave the note ending in a run of blank lines where the link was.
    while out.len() > 1 && out[out.len() - 1].is_empty() && out[out.len() - 2].is_empty() {
        out.pop();
    }
    out.join("\n")
}

/// Human-readable size: `850 B`, `12 KB`, `3.4 MB`.
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else {
        format!("{:.1} GB", b / KB / KB / KB)
    }
}

/// The line added to the note for an attachment: `- HH:MM [name](target)`.
pub fn link_line(time: chrono::NaiveTime, label: &str, target: &str) -> String {
    let target = if target.contains([' ', '(', ')', '<', '>']) { format!("<{target}>") } else { target.to_string() };
    format!("- {} [{}]({target})", time.format("%H:%M"), label.replace(['[', ']'], "_"))
}

/// A link to somewhere outside the notes (web page, mail, …).
pub fn is_url(dest: &str) -> bool {
    match dest.find(':') {
        Some(i) => {
            let scheme = &dest[..i];
            !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        None => false,
    }
}

/// The local file a link in note `note_rel` points to, if it stays inside the
/// notes folder.
pub fn resolve_link(notes_dir: &Path, note_rel: &str, dest: &str) -> Option<PathBuf> {
    if dest.is_empty() || is_url(dest) || dest.starts_with('#') || dest.starts_with('/') {
        return None;
    }
    let dest = percent_decode(dest.split(['#', '?']).next().unwrap_or(dest));
    let parent = note_rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let rel = if parent.is_empty() { dest } else { format!("{parent}/{dest}") };
    notes::resolve(notes_dir, &rel).ok()
}

/// Images the preview can show (others are listed as files to open).
pub fn is_image(path: &str) -> bool {
    matches!(ext(path).as_str(), "png" | "jpg" | "jpeg")
}

/// MIME type for opening a file in another app.
pub fn mime_for(path: &str) -> &'static str {
    match ext(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "odt" => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    }
}

/// File-name extension for a MIME type, for shared files that come without a name.
pub fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

fn ext(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_dirs() {
        assert!(is_attachment_dir("2026_11_26.md.files"));
        assert!(!is_attachment_dir("photos"));
        assert_eq!(dir_for("sub/a.md"), "sub/a.md.files");
        assert_eq!(sanitize("My photo (1).JPG"), "My_photo__1_.JPG");
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("café.pdf"), "café.pdf");
        assert_eq!(sanitize("..."), "file");
    }

    #[test]
    fn add_moves_and_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for _ in 0..2 {
            let src = d.join("incoming.tmp");
            std::fs::write(&src, b"img").unwrap();
            add(d, "2026_11_26.md", &src, "photo.jpg", Take::Move).unwrap();
            assert!(!src.exists());
        }
        assert!(d.join("2026_11_26.md.files/photo.jpg").exists());
        let src = d.join("x.tmp");
        std::fs::write(&src, b"img").unwrap();
        assert_eq!(add(d, "2026_11_26.md", &src, "photo.jpg", Take::Copy).unwrap(), "2026_11_26.md.files/photo-3.jpg");
        assert!(src.exists(), "copy keeps the original");
    }

    #[test]
    fn list_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert!(list(d, "n.md").is_empty());
        notes::write(d, "n.md", "# n\n\n- 09:01 [b.pdf](n.md.files/b.pdf)\nsee [a](n.md.files/a.jpg) here\n- 09:02 [a.jpg](n.md.files/a.jpg)\n").unwrap();
        std::fs::create_dir(d.join("n.md.files")).unwrap();
        std::fs::write(d.join("n.md.files/b.pdf"), b"pdf!").unwrap();
        std::fs::write(d.join("n.md.files/a.jpg"), b"jpg").unwrap();
        let names: Vec<_> = list(d, "n.md").into_iter().map(|a| (a.name, a.size)).collect();
        assert_eq!(names, vec![("a.jpg".into(), 3), ("b.pdf".into(), 4)]);

        // The link line goes; a link inside other text stays.
        let text = delete(d, "n.md", "a.jpg").unwrap().unwrap();
        assert_eq!(text, "# n\n\n- 09:01 [b.pdf](n.md.files/b.pdf)\nsee [a](n.md.files/a.jpg) here\n");
        assert!(!d.join("n.md.files/a.jpg").exists());

        delete(d, "n.md", "b.pdf").unwrap();
        assert!(!d.join("n.md.files").exists(), "empty folder removed");
        assert!(delete(d, "n.md", "../n.md").is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(850), "850 B");
        assert_eq!(format_size(12 * 1024), "12 KB");
        assert_eq!(format_size(3_565_158), "3.4 MB");
    }

    #[test]
    fn links() {
        let t = chrono::NaiveTime::from_hms_opt(9, 5, 0).unwrap();
        assert_eq!(link_line(t, "a.jpg", "n.md.files/a.jpg"), "- 09:05 [a.jpg](n.md.files/a.jpg)");
        assert_eq!(link_line(t, "a.jpg", "My note.md.files/a.jpg"), "- 09:05 [a.jpg](<My note.md.files/a.jpg>)");
        assert!(is_url("https://x.y") && is_url("mailto:a@b") && !is_url("n.md.files/a.jpg"));

        let d = Path::new("/notes");
        assert_eq!(resolve_link(d, "a.md", "a.md.files/p.jpg"), Some(PathBuf::from("/notes/a.md.files/p.jpg")));
        assert_eq!(resolve_link(d, "sub/a.md", "a.md.files/p%20q.jpg"), Some(PathBuf::from("/notes/sub/a.md.files/p q.jpg")));
        assert_eq!(resolve_link(d, "a.md", "../secret"), None);
        assert_eq!(resolve_link(d, "a.md", "https://x.y/p.jpg"), None);
        assert!(is_image("x/P.JPG") && !is_image("doc.pdf"));
        assert_eq!(mime_for("a.pdf"), "application/pdf");
    }
}
