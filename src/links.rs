//! Links between notes, written `[[Name]]` (or `[[Name|text]]`,
//! `[[Name#heading]]`), as in Obsidian. `Name` is a note's name without
//! `.md`, or its path for notes in subfolders.

use crate::notes;
use std::io;
use std::path::Path;

/// A `[[…]]` in some text: where it is and the note name it points to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    /// Byte range of the name inside the text (between `[[` and `|`, `#` or `]]`).
    pub start: usize,
    pub end: usize,
    pub name: String,
}

/// The `[[…]]` links in `text`, in order. Code spans and blocks aren't
/// skipped: a diary seldom has `[[` in code.
pub fn find(text: &str) -> Vec<WikiLink> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(open) = text[i..].find("[[") {
        let start = i + open + 2;
        let Some(close) = text[start..].find("]]") else { break };
        let inner = &text[start..start + close];
        if !inner.contains('\n') && !inner.contains("[[") {
            let name_len = inner.find(['|', '#']).unwrap_or(inner.len());
            let name = inner[..name_len].trim();
            if !name.is_empty() {
                let lead = inner.len() - inner.trim_start().len();
                out.push(WikiLink {
                    start: start + lead,
                    end: start + lead + name.len(),
                    name: name.to_string(),
                });
            }
        }
        i = start + close + 2;
    }
    out
}

fn stem(rel: &str) -> &str {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    file.strip_suffix(".md").unwrap_or(file)
}

fn without_md(rel: &str) -> &str {
    rel.strip_suffix(".md").unwrap_or(rel)
}

/// Does `[[name]]` point at note `rel`? By path first (`folder/Note`), else
/// by file name, ignoring case.
pub fn points_to(name: &str, rel: &str) -> bool {
    let name = name.strip_suffix(".md").unwrap_or(name);
    if name.contains('/') {
        name.eq_ignore_ascii_case(without_md(rel))
    } else {
        name.eq_ignore_ascii_case(stem(rel))
    }
}

/// The note `[[name]]`, written in workspace `from` (a folder, or `""`),
/// points to among `rels`. A path can also be relative to that workspace.
/// When several notes share the name, ones in the same workspace come
/// first, then the shallowest.
pub fn resolve<'a>(name: &str, rels: impl IntoIterator<Item = &'a str>, from: &str) -> Option<String> {
    let in_ws = (!from.is_empty() && name.contains('/')).then(|| format!("{from}/{name}"));
    let mut hits: Vec<&str> = rels
        .into_iter()
        .filter(|r| points_to(name, r) || in_ws.as_deref().is_some_and(|n| points_to(n, r)))
        .collect();
    let same = |r: &str| !from.is_empty() && r.starts_with(&format!("{from}/"));
    hits.sort_by_key(|r| (!same(r), r.matches('/').count(), r.to_string()));
    hits.first().map(|r| r.to_string())
}

/// A note that links to another one, and the line where it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub from: String,
    pub line: String,
}

/// Notes linking to `rel`, newest first, one entry per linking line.
/// `CONFIG.md` and conflict copies are left out.
pub fn backlinks(dir: &Path, rel: &str) -> Vec<Backlink> {
    let mut out = Vec::new();
    for n in notes::list(dir) {
        let from = n.rel_path;
        if from == rel || crate::calendar::is_config(&from) || from.contains(".conflict-") {
            continue;
        }
        let Ok(text) = notes::read(dir, &from) else { continue };
        if !text.contains("[[") {
            continue;
        }
        for line in text.lines() {
            if find(line).iter().any(|l| points_to(&l.name, rel)) {
                out.push(Backlink { from: from.clone(), line: line.trim().to_string() });
            }
        }
    }
    // Daily notes are named by date, so this puts the latest days first.
    out.sort_by(|a, b| b.from.cmp(&a.from));
    out
}

/// After renaming note `old` to `new`, point `[[old]]` links in every note
/// at `new`. Returns the notes changed.
pub fn rename(dir: &Path, old: &str, new: &str) -> io::Result<Vec<String>> {
    let mut changed = Vec::new();
    for n in notes::list(dir) {
        let rel = n.rel_path;
        let text = notes::read(dir, &rel)?;
        let fixed = replace_links(&text, old, new);
        if fixed != text {
            notes::write(dir, &rel, &fixed)?;
            changed.push(rel);
        }
    }
    Ok(changed)
}

/// `text` with its links to `old` pointed at `new` (a path if the link was
/// written as one, else the bare name).
pub fn replace_links(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for l in find(text) {
        if points_to(&l.name, old) {
            out.push_str(&text[last..l.start]);
            out.push_str(if l.name.contains('/') { without_md(new) } else { stem(new) });
            last = l.end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// While typing a link: if the cursor (a byte offset in `text`) follows an
/// unfinished `[[…`, where the name starts and what's typed of it so far.
pub fn pending_query(text: &str, cursor: usize) -> Option<(usize, String)> {
    let before = text.get(..cursor)?;
    let open = before.rfind("[[")?;
    let typed = &before[open + 2..];
    if typed.contains("]]") || typed.contains('\n') || typed.contains('|') || typed.contains('#') {
        return None;
    }
    Some((open + 2, typed.to_string()))
}

/// Names to offer for a link being typed in workspace `ws` (a folder, or
/// `""`): notes whose name contains `query` (ignoring case). Notes in `ws`
/// come first, named without the folder; others with it. Then names
/// starting with the query, then the order given (most recently changed
/// first). An empty query gives recent notes.
pub fn suggestions(rels: &[String], query: &str, limit: usize, ws: &str) -> Vec<String> {
    let q = query.trim().to_lowercase();
    let prefix = format!("{ws}/");
    // (in another workspace, doesn't start with the query, order) -> name
    let mut found: Vec<((bool, bool, usize), String)> = Vec::new();
    for (i, r) in rels.iter().enumerate() {
        if crate::calendar::is_config(r) || r.contains(".conflict-") {
            continue;
        }
        let own = ws.is_empty() || r.starts_with(&prefix);
        let full = without_md(r);
        let name = if ws.is_empty() { full } else { full.strip_prefix(&prefix).unwrap_or(full) };
        let lower = name.to_lowercase();
        let file = stem(r).to_lowercase();
        let starts = q.is_empty() || file.starts_with(&q) || lower.starts_with(&q);
        if starts || lower.contains(&q) {
            found.push(((!own, !starts, i), name.to_string()));
        }
    }
    found.sort_by_key(|(k, _)| *k);
    found.into_iter().take(limit).map(|(_, n)| n).collect()
}

/// Finish the link being typed (name from `start` to `cursor`) with `name`
/// and its `]]` (unless already there). Returns the text and where the
/// cursor goes: after the `]]`.
pub fn complete(text: &str, start: usize, cursor: usize, name: &str) -> (String, usize) {
    let closed = text[cursor..].starts_with("]]");
    let mut out = String::with_capacity(text.len() + name.len() + 2);
    out.push_str(&text[..start]);
    out.push_str(name);
    if !closed {
        out.push_str("]]");
    }
    let after = start + name.len() + 2;
    out.push_str(&text[cursor..]);
    (out, after)
}

/// A line for showing: without its list marker or task box, and with
/// `[[Name|text]]` shown as `text` (`[[Name]]` as `Name`).
pub fn plain(line: &str) -> String {
    let mut l = line.trim();
    for p in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* "] {
        if let Some(rest) = l.strip_prefix(p) {
            l = rest;
            break;
        }
    }
    let mut out = String::with_capacity(l.len());
    let mut rest = l;
    while let Some(open) = rest.find("[[") {
        let Some(close) = rest[open..].find("]]") else { break };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 2..open + close];
        let shown = inner.split_once('|').map_or(inner.split('#').next().unwrap_or(inner), |(_, t)| t);
        out.push_str(shown.trim());
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links() {
        let t = "Vaig anar a [[Montjoia]] amb [[Aran|l'Aran]] i [[ projectes/diari#idees ]]. [[]] [[x\ny]]";
        let names: Vec<_> = find(t).into_iter().map(|l| l.name).collect();
        assert_eq!(names, vec!["Montjoia", "Aran", "projectes/diari"]);
        let l = &find(t)[0];
        assert_eq!(&t[l.start..l.end], "Montjoia");
    }

    #[test]
    fn completion() {
        let t = "Anar a [[Mon";
        assert_eq!(pending_query(t, t.len()), Some((9, "Mon".into())));
        assert_eq!(pending_query("a [[x]] b", 9), None);
        assert_eq!(pending_query("a [[x\nb", 7), None);
        assert_eq!(pending_query("res", 3), None);
        assert_eq!(complete(t, 9, t.len(), "Montjoia"), ("Anar a [[Montjoia]]".into(), 19));
        // Already closed (the cursor before "]]"): don't add another.
        let t = "[[Mo]] i";
        assert_eq!(complete(t, 2, 4, "Montjoia"), ("[[Montjoia]] i".into(), 12));

        let rels: Vec<String> =
            ["2026_11_27.md", "Montjoia.md", "projectes/Mon diari.md", "CONFIG.md", "Simone.md"].map(String::from).into();
        assert_eq!(suggestions(&rels, "mon", 5, ""), vec!["Montjoia", "projectes/Mon diari", "Simone"]);
        assert_eq!(suggestions(&rels, "", 2, ""), vec!["2026_11_27", "Montjoia"]);
        let rels: Vec<String> =
            ["diari/2026_11_20.md", "feina/Projecte X.md", "diari/Projectes.md", "feina/CONFIG.md"].map(String::from).into();
        assert_eq!(suggestions(&rels, "proj", 5, "feina"), vec!["Projecte X", "diari/Projectes"]);
        assert_eq!(suggestions(&rels, "2026", 5, "feina"), vec!["diari/2026_11_20"]);
    }

    #[test]
    fn plain_lines() {
        assert_eq!(plain("- 10:00 Cap a [[Montjoia]]"), "10:00 Cap a Montjoia");
        assert_eq!(plain("- [ ] Netejar [[Montjoia|la casa]] i [[x#y]]"), "Netejar la casa i x");
    }

    #[test]
    fn resolves_by_path_then_name() {
        let rels = ["Montjoia.md", "projectes/diari.md", "arxiu/Montjoia.md"];
        assert_eq!(resolve("montjoia", rels, "").as_deref(), Some("Montjoia.md"));
        assert_eq!(resolve("diari", rels, "").as_deref(), Some("projectes/diari.md"));
        assert_eq!(resolve("arxiu/Montjoia", rels, "").as_deref(), Some("arxiu/Montjoia.md"));
        assert_eq!(resolve("Ningú", rels, ""), None);
        // From a workspace: its own notes first, and paths relative to it.
        let rels = ["diari/Casa.md", "feina/Casa.md", "feina/clients/Acme.md"];
        assert_eq!(resolve("Casa", rels, "feina").as_deref(), Some("feina/Casa.md"));
        assert_eq!(resolve("Casa", rels, "diari").as_deref(), Some("diari/Casa.md"));
        assert_eq!(resolve("clients/Acme", rels, "feina").as_deref(), Some("feina/clients/Acme.md"));
    }

    #[test]
    fn renames_links_keeping_their_text() {
        let t = "- [[Montjoia]] i [[Montjoia|la casa]] i [[Montjoia#llista]] i [[Altres]]";
        assert_eq!(
            replace_links(t, "Montjoia.md", "Casa Moianès.md"),
            "- [[Casa Moianès]] i [[Casa Moianès|la casa]] i [[Casa Moianès#llista]] i [[Altres]]"
        );
        assert_eq!(replace_links("[[a/b]]", "a/b.md", "c/d.md"), "[[c/d]]");
    }

    #[test]
    fn backlinks_and_rename_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        notes::write(p, "Montjoia.md", "# Montjoia\n").unwrap();
        notes::write(p, "2026_11_20.md", "# 2026_11_20\n\n- 10:00 Cap a [[Montjoia]]\n- res\n").unwrap();
        notes::write(p, "2026_11_27.md", "# 2026_11_27\n\n- [[montjoia|la casa]] neta\n").unwrap();
        notes::write(p, "CONFIG.md", "[[Montjoia]]\n").unwrap();
        let b = backlinks(p, "Montjoia.md");
        assert_eq!(
            b,
            vec![
                Backlink { from: "2026_11_27.md".into(), line: "- [[montjoia|la casa]] neta".into() },
                Backlink { from: "2026_11_20.md".into(), line: "- 10:00 Cap a [[Montjoia]]".into() },
            ]
        );
        let mut changed = rename(p, "Montjoia.md", "Casa.md").unwrap();
        changed.sort();
        assert_eq!(changed, vec!["2026_11_20.md", "2026_11_27.md", "CONFIG.md"]);
        assert!(notes::read(p, "2026_11_27.md").unwrap().contains("[[Casa|la casa]]"));
    }
}
