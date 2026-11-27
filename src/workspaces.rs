//! Workspaces: top-level folders of the notes repo that have their own
//! `CONFIG.md`. `diari/` is the journal, the only one with daily notes
//! (`diari/2026_11_27.md`); the others (`feina/`, `familia/`…) hold notes
//! on a subject.
//!
//! Before workspaces, every note lived at the top of the repo. Until those
//! notes are moved into `diari/` (`migrate_root`), the top is the journal.

use crate::{attachments, calendar, notes};
use serde::Deserialize;
use std::io;
use std::path::Path;

/// The journal's folder.
pub const JOURNAL: &str = "diari";

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    /// Folder at the top of the repo.
    pub folder: String,
    /// Shown name: `name:` in the `# workspace` section of its CONFIG.md,
    /// else the folder's name capitalized.
    pub name: String,
    /// `color:` there, e.g. `#3b82f6`.
    pub color: Option<String>,
}

#[derive(Deserialize, Default)]
struct Settings {
    name: Option<String>,
    color: Option<String>,
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The workspaces, the journal first, then by name.
pub fn list(dir: &Path) -> Vec<Workspace> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Workspace> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder.starts_with('.') || attachments::is_attachment_dir(&folder) {
                return None;
            }
            let config = calendar::config_path(&e.path())?;
            let text = std::fs::read_to_string(config).unwrap_or_default();
            let settings: Settings = calendar::yaml_section(&text, "workspace")
                .and_then(|y| serde_yaml::from_str(&y).ok())
                .unwrap_or_default();
            Some(Workspace {
                name: settings.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| capitalized(&folder)),
                color: settings.color,
                folder,
            })
        })
        .collect();
    out.sort_by(|a, b| (a.folder != JOURNAL, a.name.to_lowercase()).cmp(&(b.folder != JOURNAL, b.name.to_lowercase())));
    out
}

/// Have the notes been moved into workspaces (is there a `diari/` one)?
pub fn enabled(dir: &Path) -> bool {
    calendar::config_path(&dir.join(JOURNAL)).is_some()
}

/// The journal's folder: `diari`, or `""` (the top) before workspaces.
pub fn journal(dir: &Path) -> &'static str {
    if enabled(dir) { JOURNAL } else { "" }
}

/// `rel` with the journal folder in front (`2026_11_27.md` -> `diari/…`).
pub fn in_journal(dir: &Path, rel: &str) -> String {
    match journal(dir) {
        "" => rel.to_string(),
        j => format!("{j}/{rel}"),
    }
}

/// The workspace folder a note is in (`""` for notes at the top).
pub fn folder_of(rel: &str) -> &str {
    rel.split_once('/').map_or("", |(f, _)| f)
}

/// Do the notes at the top still need moving into `diari/`?
pub fn needs_migration(dir: &Path) -> bool {
    !enabled(dir) && top_entries(dir).iter().any(|n| n.to_ascii_lowercase().ends_with(".md"))
}

/// What's at the top of the repo that belongs to the journal: everything
/// but hidden entries and workspace folders.
fn top_entries(dir: &Path) -> Vec<String> {
    let workspaces: Vec<String> = list(dir).into_iter().map(|w| w.folder).collect();
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && !workspaces.contains(n) && n != JOURNAL)
        .collect();
    out.sort();
    out
}

/// Move every note (and attachment folder, and CONFIG.md) at the top into
/// `diari/`, which becomes the journal workspace. Returns how many entries
/// moved. Links keep working: attachment links are relative to their note,
/// and `[[Name]]` goes by name.
pub fn migrate_root(dir: &Path) -> io::Result<usize> {
    let journal = dir.join(JOURNAL);
    std::fs::create_dir_all(&journal)?;
    let entries = top_entries(dir);
    for name in &entries {
        let to = journal.join(name);
        if to.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{JOURNAL}/{name} already exists")));
        }
    }
    for name in &entries {
        std::fs::rename(dir.join(name), journal.join(name))?;
    }
    if calendar::config_path(&journal).is_none() {
        notes::atomic_write(&journal.join("CONFIG.md"), b"# CONFIG\n")?;
    }
    Ok(entries.len())
}

/// After workspaces: notes that turned up at the top again (written by a
/// device with an older diari) go into `diari/`. A note that's already
/// there gets the new lines appended instead. Returns the notes moved.
pub fn adopt_strays(dir: &Path) -> io::Result<Vec<String>> {
    if !enabled(dir) {
        return Ok(Vec::new());
    }
    let mut moved = Vec::new();
    for name in top_entries(dir) {
        let from = dir.join(&name);
        let to = dir.join(JOURNAL).join(&name);
        let is_note = name.to_ascii_lowercase().ends_with(".md") && from.is_file();
        if !is_note && !(attachments::is_attachment_dir(&name) && from.is_dir()) {
            continue; // leave anything else (README, scripts…) alone
        }
        if !to.exists() {
            std::fs::rename(&from, &to)?;
        } else if is_note {
            let old = std::fs::read_to_string(&to)?;
            let new = std::fs::read_to_string(&from)?;
            let missing: Vec<&str> =
                new.lines().filter(|l| !l.trim().is_empty() && !old.lines().any(|o| o == *l)).collect();
            if !missing.is_empty() {
                let mut merged = old.trim_end().to_string();
                merged.push_str("\n\n");
                merged.push_str(&missing.join("\n"));
                merged.push('\n');
                notes::atomic_write(&to, merged.as_bytes())?;
            }
            std::fs::remove_file(&from)?;
        } else {
            // Attachment folder: move its files over, keeping both on clashes.
            for f in std::fs::read_dir(&from)?.flatten() {
                let mut target = to.join(f.file_name());
                if target.exists() {
                    target = to.join(format!("old-{}", f.file_name().to_string_lossy()));
                }
                std::fs::rename(f.path(), target)?;
            }
            let _ = std::fs::remove_dir(&from);
        }
        moved.push(name);
    }
    Ok(moved)
}

/// Make a workspace named `name`: a folder (its name in lower case, spaces
/// as `-`) with a CONFIG.md holding the name. Returns the folder.
pub fn create(dir: &Path, name: &str) -> io::Result<String> {
    let name = name.trim();
    let folder: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if folder.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty name"));
    }
    let path = dir.join(&folder);
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{folder} already exists")));
    }
    std::fs::create_dir_all(&path)?;
    let config = format!("# CONFIG\n\n# workspace\n```yaml\nname: \"{}\"\n```\n", name.replace('"', "'"));
    notes::atomic_write(&path.join("CONFIG.md"), config.as_bytes())?;
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, rel: &str, text: &str) {
        let f = p.join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }

    #[test]
    fn migrate_then_adopt() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(p, "2026_11_27.md", "# 2026_11_27\n\n- 10:00 [foto](2026_11_27.md.files/f.jpg)\n");
        write(p, "2026_11_27.md.files/f.jpg", "jpg");
        write(p, "CONFIG.md", "# cal\n");
        write(p, "projectes/Casa.md", "# Casa\n");
        write(p, ".git/HEAD", "x");
        assert!(needs_migration(p));
        assert_eq!(journal(p), "");
        assert_eq!(in_journal(p, "2026_11_27.md"), "2026_11_27.md");

        assert_eq!(migrate_root(p).unwrap(), 4);
        assert!(!needs_migration(p));
        assert_eq!(journal(p), "diari");
        assert!(p.join("diari/2026_11_27.md.files/f.jpg").exists());
        assert!(p.join("diari/projectes/Casa.md").exists());
        assert!(p.join(".git/HEAD").exists());
        assert_eq!(in_journal(p, "2026_11_27.md"), "diari/2026_11_27.md");

        // An older diari elsewhere wrote today's note at the top again.
        write(p, "2026_11_27.md", "# 2026_11_27\n\n- 11:00 des del mòbil\n");
        write(p, "2026_11_28.md", "# 2026_11_28\n");
        let mut moved = adopt_strays(p).unwrap();
        moved.sort();
        assert_eq!(moved, vec!["2026_11_27.md", "2026_11_28.md"]);
        let merged = std::fs::read_to_string(p.join("diari/2026_11_27.md")).unwrap();
        assert!(merged.contains("[foto]") && merged.ends_with("- 11:00 des del mòbil\n"), "{merged}");
        assert!(p.join("diari/2026_11_28.md").exists() && !p.join("2026_11_28.md").exists());
    }

    #[test]
    fn workspaces_and_create() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        write(p, "diari/CONFIG.md", "# cal\n");
        write(p, "zeta/CONFIG.md", "# workspace\n```yaml\nname: Família\ncolor: \"#e11d48\"\n```\n");
        write(p, "notaworkspace/x.md", "x");
        assert_eq!(create(p, "La Feina").unwrap(), "la-feina");
        assert!(create(p, "la feina").is_err());
        let ws = list(p);
        let got: Vec<_> = ws.iter().map(|w| (w.folder.as_str(), w.name.as_str(), w.color.as_deref())).collect();
        assert_eq!(
            got,
            vec![("diari", "Diari", None), ("zeta", "Família", Some("#e11d48")), ("la-feina", "La Feina", None)]
        );
        assert_eq!(folder_of("feina/Projecte.md"), "feina");
        assert_eq!(folder_of("x.md"), "");
    }
}
