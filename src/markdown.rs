//! Markdown -> flat list of styled blocks rendered by native Slint widgets.

use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Para,
    Heading(u8),
    Code,
    Quote,
    ListItem,
    /// A task list item (`- [ ] …` / `- [x] …`).
    Task { done: bool },
    Rule,
    /// The title row of a callout (`> [!warning] Title`); the blocks inside
    /// it follow, with the same `callout`.
    CalloutTitle,
    /// A link (`[label](target)` or `![alt](target)`), shown below the text
    /// it appears in: the app shows local images and opens everything else.
    Link,
}

impl Kind {
    /// Integer encoding shared with `ui/app.slint` (MdBlock.kind).
    pub fn code(self) -> i32 {
        match self {
            Kind::Para => 0,
            Kind::Heading(l) => l as i32,
            Kind::Code => 7,
            Kind::Quote => 8,
            Kind::ListItem => 9,
            Kind::Task { .. } => 13,
            Kind::Rule => 10,
            Kind::Link => 12,
            Kind::CalloutTitle => 14,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub text: String,
    pub indent: i32,
    /// Link target, for `Kind::Link`.
    pub target: String,
    /// `Kind::Link` written as an image (`![alt](target)`).
    pub image: bool,
    /// `Kind::Task`: byte offset in the source of its `[ ]` / `[x]`.
    /// In a callout: where the callout starts (identifies it).
    pub pos: usize,
    /// The callout this block is in (its title row included), if any.
    pub callout: Option<Callout>,
}

/// An Obsidian callout: a quote whose first line is `[!type]`, optionally
/// followed by `-` (folded) or `+` (foldable, open) and a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Callout {
    pub color: CalloutColor,
    /// Byte offset in the source where the callout starts.
    pub pos: usize,
    /// `Some(folded)` if it can be folded: folded at first with `-`.
    pub fold: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutColor {
    Blue,
    Green,
    Orange,
    Red,
    Purple,
    Grey,
}

impl CalloutColor {
    /// Integer encoding shared with `ui/app.slint` (MdBlock.callout).
    pub fn code(self) -> i32 {
        match self {
            CalloutColor::Blue => 1,
            CalloutColor::Green => 2,
            CalloutColor::Orange => 3,
            CalloutColor::Red => 4,
            CalloutColor::Purple => 5,
            CalloutColor::Grey => 6,
        }
    }
}

/// Highlights (`==text==`) in a piece of text: byte ranges from the opening
/// `==` to after the closing one. The text can't start or end with a space,
/// and a run of three or more `=` isn't a marker.
pub fn highlight_ranges(s: &str) -> Vec<std::ops::Range<usize>> {
    let b = s.as_bytes();
    let is_marker = |i: usize| {
        i + 1 < b.len() && b[i] == b'=' && b[i + 1] == b'=' && (i == 0 || b[i - 1] != b'=') && b.get(i + 2) != Some(&b'=')
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if is_marker(i) && b.get(i + 2).is_some_and(|c| !c.is_ascii_whitespace()) {
            let close = (i + 3..b.len().saturating_sub(1)).find(|&j| is_marker(j) && !b[j - 1].is_ascii_whitespace());
            if let Some(j) = close {
                out.push(i..j + 2);
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The text without its highlight markers (the preview shows plain text).
fn strip_highlights(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for r in highlight_ranges(s) {
        out.push_str(&s[last..r.start]);
        out.push_str(&s[r.start + 2..r.end - 2]);
        last = r.end;
    }
    out.push_str(&s[last..]);
    out
}

/// The colour of a callout type.
pub fn callout_color(kind: &str) -> CalloutColor {
    callout_style(kind).0
}

/// The type's colour and icon (Obsidian's types and their aliases; unknown
/// types look like a note).
fn callout_style(kind: &str) -> (CalloutColor, &'static str) {
    use CalloutColor::*;
    match kind {
        "abstract" | "summary" | "tldr" => (Blue, "📄"),
        "info" => (Blue, "ℹ️"),
        "todo" => (Blue, "☑️"),
        "tip" | "hint" | "important" => (Green, "💡"),
        "success" | "check" | "done" => (Green, "✅"),
        "question" | "help" | "faq" => (Orange, "❓"),
        "warning" | "caution" | "attention" => (Orange, "⚠️"),
        "failure" | "fail" | "missing" => (Red, "❌"),
        "danger" | "error" => (Red, "⛔"),
        "bug" => (Red, "🐞"),
        "example" => (Purple, "📋"),
        "quote" | "cite" => (Grey, "❝"),
        _ => (Blue, "📝"),
    }
}

/// A callout's first line (`[!type]-  Title`, after the `>`): its type,
/// fold marker and title. The title defaults to the type's name.
pub fn parse_callout_line(line: &str) -> Option<(String, Option<bool>, String)> {
    let rest = line.trim_start().strip_prefix("[!")?;
    let end = rest.find(']')?;
    let kind = rest[..end].trim().to_lowercase();
    if kind.is_empty() || kind.contains(char::is_whitespace) {
        return None;
    }
    let mut rest = &rest[end + 1..];
    let fold = match rest.chars().next() {
        Some('-') => Some(true),
        Some('+') => Some(false),
        _ => None,
    };
    if fold.is_some() {
        rest = &rest[1..];
    }
    let mut title = rest.trim().to_string();
    if title.is_empty() {
        let mut c = kind.chars();
        title = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    }
    Some((kind, fold, title))
}

struct State {
    out: Vec<Block>,
    buf: String,
    cur: Option<(Kind, i32)>,
    lists: Vec<Option<u64>>,
    quote: i32,
    /// Links in the current block: (target, label, is image), emitted after it.
    links: Vec<(String, String, bool)>,
    /// Open link: its target, where its label starts in `buf`, is an image.
    open_link: Option<(String, usize, bool)>,
    /// Offset of the current block's task marker.
    task_pos: usize,
    /// The callout being read, and the quote depth it started at.
    callout: Option<(Callout, i32)>,
    /// The callout's first line (its `[!type] title`) is still in the text
    /// to come and must be left out.
    skip_callout_line: bool,
}

impl State {
    fn flush(&mut self) {
        if let Some((kind, indent)) = self.cur.take() {
            let text = if kind == Kind::Code {
                self.buf.trim_end_matches('\n').to_string()
            } else {
                self.buf.trim().to_string()
            };
            let text = if self.skip_callout_line && kind != Kind::Code {
                self.skip_callout_line = false;
                text.split_once('\n').map_or(String::new(), |(_, rest)| rest.trim().to_string())
            } else {
                text
            };
            let text = if kind == Kind::Code { text } else { strip_highlights(&text) };
            let callout = self.callout.map(|(c, _)| c);
            if !text.is_empty() || matches!(kind, Kind::Task { .. }) {
                let pos = match (kind, callout) {
                    (Kind::Task { .. }, _) => self.task_pos,
                    (_, Some(c)) => c.pos,
                    _ => 0,
                };
                self.out.push(Block { kind, text, indent, target: String::new(), image: false, pos, callout });
            }
            for (target, label, image) in self.links.drain(..) {
                let pos = callout.map_or(0, |c| c.pos);
                self.out.push(Block { kind: Kind::Link, text: label, indent, target, image, pos, callout });
            }
        }
        self.buf.clear();
    }

    fn start_link(&mut self, target: &str, image: bool) {
        self.ensure_open();
        self.open_link = Some((target.to_string(), self.buf.len(), image));
    }

    fn end_link(&mut self) {
        if let Some((target, start, image)) = self.open_link.take() {
            let label = self.buf[start..].trim().to_string();
            // A picture's alt text is its caption, not part of the sentence.
            if image {
                self.buf.truncate(start);
            }
            let label = if label.is_empty() { target.rsplit('/').next().unwrap_or(&target).to_string() } else { label };
            self.links.push((target, label, image));
        }
    }

    /// Quote levels that indent: the callout's own doesn't.
    fn quote_depth(&self) -> i32 {
        self.quote - self.callout.map_or(0, |(_, d)| d)
    }

    fn indent(&self) -> i32 {
        self.lists.len().saturating_sub(1) as i32 + self.quote_depth()
    }

    fn ensure_open(&mut self) {
        if self.cur.is_none() {
            let q = self.quote_depth();
            let kind = if q > 0 { Kind::Quote } else { Kind::Para };
            let indent = if self.lists.is_empty() { (q - 1).max(0) } else { self.lists.len() as i32 };
            self.cur = Some((kind, indent));
        }
    }
}

pub fn parse(src: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_WIKILINKS;
    let mut s = State {
        out: Vec::new(),
        buf: String::new(),
        cur: None,
        lists: Vec::new(),
        quote: 0,
        links: Vec::new(),
        open_link: None,
        task_pos: 0,
        callout: None,
        skip_callout_line: false,
    };

    for (ev, range) in Parser::new_ext(src, opts).into_offset_iter() {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    s.flush();
                    let l = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    s.cur = Some((Kind::Heading(l), 0));
                }
                Tag::Paragraph => s.ensure_open(),
                Tag::CodeBlock(_) => {
                    s.flush();
                    s.cur = Some((Kind::Code, s.indent()));
                }
                Tag::BlockQuote(_) => {
                    s.flush();
                    s.quote += 1;
                    // A callout? Its first line, after the `>`s.
                    if s.callout.is_none() {
                        let first = src[range.clone()].lines().next().unwrap_or("");
                        let line = first.trim_start().trim_start_matches(['>', ' ']);
                        if let Some((kind, fold, title)) = parse_callout_line(line) {
                            let (color, icon) = callout_style(&kind);
                            let c = Callout { color, pos: range.start, fold };
                            s.out.push(Block {
                                kind: Kind::CalloutTitle,
                                text: format!("{icon} {title}"),
                                indent: (s.quote - 1).max(0) + s.lists.len() as i32,
                                target: String::new(),
                                image: false,
                                pos: range.start,
                                callout: Some(c),
                            });
                            s.callout = Some((c, s.quote));
                            s.skip_callout_line = true;
                        }
                    }
                }
                Tag::List(start) => {
                    s.flush();
                    s.lists.push(start);
                }
                Tag::Item => {
                    s.flush();
                    let indent = s.indent();
                    let prefix = match s.lists.last_mut() {
                        Some(Some(n)) => {
                            let p = format!("{n}. ");
                            *n += 1;
                            p
                        }
                        _ => "• ".to_string(),
                    };
                    s.cur = Some((Kind::ListItem, indent));
                    s.buf.push_str(&prefix);
                }
                Tag::Table(_) => s.flush(),
                Tag::TableHead | Tag::TableRow => {
                    s.flush();
                    s.cur = Some((Kind::Code, s.indent()));
                }
                // A link to a note: its target is kept as `[[Name]]`.
                Tag::Link { link_type: LinkType::WikiLink { .. }, dest_url, .. } => {
                    s.start_link(&format!("[[{dest_url}]]"), false)
                }
                Tag::Link { dest_url, .. } => s.start_link(&dest_url, false),
                Tag::Image { dest_url, .. } => s.start_link(&dest_url, true),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item | TagEnd::Paragraph => s.flush(),
                TagEnd::BlockQuote(_) => {
                    s.flush();
                    if s.callout.is_some_and(|(_, d)| d == s.quote) {
                        s.callout = None;
                        s.skip_callout_line = false;
                    }
                    s.quote -= 1;
                }
                TagEnd::List(_) => {
                    s.flush();
                    s.lists.pop();
                }
                TagEnd::TableCell => s.buf.push_str(" | "),
                TagEnd::TableHead | TagEnd::TableRow => {
                    let trimmed = s.buf.trim_end().trim_end_matches('|').trim_end().to_string();
                    s.buf = trimmed;
                    s.flush();
                }
                TagEnd::Link | TagEnd::Image => s.end_link(),
                _ => {}
            },
            Event::Text(t) => {
                s.ensure_open();
                s.buf.push_str(&t);
            }
            Event::Code(t) => {
                s.ensure_open();
                s.buf.push_str(&t);
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                s.ensure_open();
                s.buf.push_str(&t);
            }
            // Show line breaks as written (a diary, not a typeset document).
            Event::SoftBreak => s.buf.push('\n'),
            Event::HardBreak => s.buf.push('\n'),
            Event::Rule => {
                s.flush();
                s.out.push(Block {
                    kind: Kind::Rule,
                    text: String::new(),
                    indent: 0,
                    target: String::new(),
                    image: false,
                    pos: 0,
                    callout: s.callout.map(|(c, _)| c),
                });
            }
            // The item becomes a task: drop its bullet, keep where the box is.
            Event::TaskListMarker(done) => {
                if let Some((kind @ Kind::ListItem, _)) = s.cur.as_mut() {
                    *kind = Kind::Task { done };
                    s.buf.clear();
                    s.task_pos = range.start;
                } else {
                    s.buf.push_str(if done { "[x] " } else { "[ ] " });
                }
            }
            Event::FootnoteReference(r) => {
                s.buf.push('[');
                s.buf.push_str(&r);
                s.buf.push(']');
            }
            _ => {}
        }
    }
    s.flush();
    s.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, String, i32)> {
        parse(src).into_iter().map(|b| (b.kind, b.text, b.indent)).collect()
    }

    #[test]
    fn blocks() {
        let src = "# Title\n\nSome *em* and `code`\nnext line.\n\n- a\n- b\n  - c\n\n1. one\n2. two\n\n> quoted\n\n```rust\nfn x() {}\n```\n\n---\n- [x] done\n";
        let got = kinds(src);
        assert_eq!(
            got,
            vec![
                (Kind::Heading(1), "Title".into(), 0),
                (Kind::Para, "Some em and code\nnext line.".into(), 0),
                (Kind::ListItem, "• a".into(), 0),
                (Kind::ListItem, "• b".into(), 0),
                (Kind::ListItem, "• c".into(), 1),
                (Kind::ListItem, "1. one".into(), 0),
                (Kind::ListItem, "2. two".into(), 0),
                (Kind::Quote, "quoted".into(), 0),
                (Kind::Code, "fn x() {}".into(), 0),
                (Kind::Rule, "".into(), 0),
                (Kind::Task { done: true }, "done".into(), 0),
            ]
        );
    }

    #[test]
    fn links_follow_their_block() {
        let got: Vec<_> = parse("- 09:05 [photo.jpg](n.md.files/photo.jpg)\n\nsee ![](a.png) and [site](https://x.y)\n")
            .into_iter()
            .map(|b| (b.kind, b.text, b.target))
            .collect();
        assert_eq!(
            got,
            vec![
                (Kind::ListItem, "• 09:05 photo.jpg".into(), "".into()),
                (Kind::Link, "photo.jpg".into(), "n.md.files/photo.jpg".into()),
                (Kind::Para, "see  and site".into(), "".into()),
                (Kind::Link, "a.png".into(), "a.png".into()),
                (Kind::Link, "site".into(), "https://x.y".into()),
            ]
        );
    }

    #[test]
    fn multi_line_entry_stays_one_item() {
        let got = kinds("- 09:05 Title\n  line two\n\n  - item\n- 10:00 next\n");
        assert_eq!(
            got,
            vec![
                (Kind::ListItem, "• 09:05 Title\nline two".into(), 0),
                (Kind::ListItem, "• item".into(), 1),
                (Kind::ListItem, "• 10:00 next".into(), 0),
            ]
        );
    }

    #[test]
    fn tasks_know_where_their_box_is() {
        let src = "# T\n\n- [ ] buy bread\n- [x] call\n  - [ ] nested\n- plain\n";
        let blocks = parse(src);
        let tasks: Vec<_> = blocks
            .iter()
            .filter(|b| matches!(b.kind, Kind::Task { .. }))
            .map(|b| (b.kind, b.text.as_str(), &src[b.pos..b.pos + 3]))
            .collect();
        assert_eq!(
            tasks,
            vec![
                (Kind::Task { done: false }, "buy bread", "[ ]"),
                (Kind::Task { done: true }, "call", "[x]"),
                (Kind::Task { done: false }, "nested", "[ ]"),
            ]
        );
        assert_eq!(blocks.last().unwrap().text, "• plain");
    }

    #[test]
    fn images_are_marked() {
        let b = parse("Look: ![cat](https://x.y/cat)\n\n![](b.png)\n\n[doc](a.pdf)\n");
        // The alt text isn't repeated as text, and a lone picture leaves no
        // empty paragraph.
        assert_eq!(b[0].text, "Look:");
        assert_eq!(b.iter().filter(|b| b.kind == Kind::Para).count(), 2);
        let links: Vec<_> = b.iter().filter(|b| b.kind == Kind::Link).map(|b| (b.target.as_str(), b.image)).collect();
        assert_eq!(links, vec![("https://x.y/cat", true), ("b.png", true), ("a.pdf", false)]);
    }

    #[test]
    fn note_links() {
        let b = parse("Anar a [[Montjoia]] amb [[Aran|l'Aran]]\n");
        assert_eq!(b[0].text, "Anar a Montjoia amb l'Aran");
        let links: Vec<_> = b.iter().filter(|b| b.kind == Kind::Link).map(|b| (b.text.as_str(), b.target.as_str())).collect();
        assert_eq!(links, vec![("Montjoia", "[[Montjoia]]"), ("l'Aran", "[[Aran]]")]);
    }

    #[test]
    fn callouts() {
        let src = "Before\n\n> [!warning] Compte\n> El servidor es reinicia.\n> - desa\n\n> [!tip]-\n> Amagat\n\n> [!nota]\n\n> just a quote\n";
        let b = parse(src);
        let got: Vec<_> = b.iter().map(|b| (b.kind, b.text.as_str(), b.indent, b.callout.map(|c| (c.color, c.fold)))).collect();
        use CalloutColor::*;
        assert_eq!(
            got,
            vec![
                (Kind::Para, "Before", 0, None),
                (Kind::CalloutTitle, "⚠️ Compte", 0, Some((Orange, None))),
                (Kind::Para, "El servidor es reinicia.", 0, Some((Orange, None))),
                (Kind::ListItem, "• desa", 0, Some((Orange, None))),
                (Kind::CalloutTitle, "💡 Tip", 0, Some((Green, Some(true)))),
                (Kind::Para, "Amagat", 0, Some((Green, Some(true)))),
                (Kind::CalloutTitle, "📝 Nota", 0, Some((Blue, None))),
                (Kind::Quote, "just a quote", 0, None),
            ]
        );
        // Blocks know their callout by where it starts.
        assert_eq!(&src[b[1].pos..b[1].pos + 2], "> ");
        assert!(b[1..4].iter().all(|x| x.pos == b[1].pos));
    }

    #[test]
    fn highlights() {
        assert_eq!(highlight_ranges("a ==b c== d ==e=="), vec![2..9, 12..17]);
        assert!(highlight_ranges("== no == ===x=== a==").is_empty());
        let b = parse("Some ==marked== text\n\n```\n==code==\n```\n");
        assert_eq!(b[0].text, "Some marked text");
        assert_eq!(b[1].text, "==code==");
    }

    #[test]
    fn callout_lines() {
        assert_eq!(parse_callout_line("[!NOTE]"), Some(("note".into(), None, "Note".into())));
        assert_eq!(parse_callout_line("[!faq]+ Per què?"), Some(("faq".into(), Some(false), "Per què?".into())));
        assert_eq!(parse_callout_line("[! two words]"), None);
        assert_eq!(parse_callout_line("[link]"), None);
    }

    #[test]
    fn table() {
        let got = kinds("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert_eq!(
            got,
            vec![(Kind::Code, "a | b".into(), 0), (Kind::Code, "1 | 2".into(), 0)]
        );
    }
}
