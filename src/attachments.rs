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

/// Move (or copy, across filesystems) `src` into the note's attachment
/// folder as `name`, adding `-2`, `-3`… if taken. Returns the link target,
/// relative to the note: `2026_11_26.md.files/photo.jpg`.
pub fn add(notes_dir: &Path, note_rel: &str, src: &Path, name: &str) -> io::Result<String> {
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
    if std::fs::rename(src, &dest).is_err() {
        std::fs::copy(src, &dest)?;
        let _ = std::fs::remove_file(src);
    }
    let folder_name = folder_rel.rsplit('/').next().unwrap_or(&folder_rel);
    Ok(format!("{folder_name}/{candidate}"))
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
            add(d, "2026_11_26.md", &src, "photo.jpg").unwrap();
            assert!(!src.exists());
        }
        assert!(d.join("2026_11_26.md.files/photo.jpg").exists());
        let src = d.join("x.tmp");
        std::fs::write(&src, b"img").unwrap();
        assert_eq!(add(d, "2026_11_26.md", &src, "photo.jpg").unwrap(), "2026_11_26.md.files/photo-3.jpg");
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
