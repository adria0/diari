//! Markdown -> flat list of styled blocks rendered by native Slint widgets.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Para,
    Heading(u8),
    Code,
    Quote,
    ListItem,
    Rule,
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
            Kind::Rule => 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub text: String,
    pub indent: i32,
}

struct State {
    out: Vec<Block>,
    buf: String,
    cur: Option<(Kind, i32)>,
    lists: Vec<Option<u64>>,
    quote: i32,
}

impl State {
    fn flush(&mut self) {
        if let Some((kind, indent)) = self.cur.take() {
            let text = if kind == Kind::Code {
                self.buf.trim_end_matches('\n').to_string()
            } else {
                self.buf.trim().to_string()
            };
            if !text.is_empty() {
                self.out.push(Block { kind, text, indent });
            }
        }
        self.buf.clear();
    }

    fn indent(&self) -> i32 {
        self.lists.len().saturating_sub(1) as i32 + self.quote
    }

    fn ensure_open(&mut self) {
        if self.cur.is_none() {
            let kind = if self.quote > 0 { Kind::Quote } else { Kind::Para };
            let indent = if self.lists.is_empty() { (self.quote - 1).max(0) } else { self.lists.len() as i32 };
            self.cur = Some((kind, indent));
        }
    }
}

pub fn parse(src: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES;
    let mut s = State { out: Vec::new(), buf: String::new(), cur: None, lists: Vec::new(), quote: 0 };

    for ev in Parser::new_ext(src, opts) {
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
                Tag::Image { .. } => s.buf.push_str("[image: "),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item | TagEnd::Paragraph => s.flush(),
                TagEnd::BlockQuote(_) => {
                    s.flush();
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
                TagEnd::Image => s.buf.push(']'),
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
            Event::SoftBreak => s.buf.push(' '),
            Event::HardBreak => s.buf.push('\n'),
            Event::Rule => {
                s.flush();
                s.out.push(Block { kind: Kind::Rule, text: String::new(), indent: 0 });
            }
            Event::TaskListMarker(done) => s.buf.push_str(if done { "[x] " } else { "[ ] " }),
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
                (Kind::Para, "Some em and code next line.".into(), 0),
                (Kind::ListItem, "• a".into(), 0),
                (Kind::ListItem, "• b".into(), 0),
                (Kind::ListItem, "• c".into(), 1),
                (Kind::ListItem, "1. one".into(), 0),
                (Kind::ListItem, "2. two".into(), 0),
                (Kind::Quote, "quoted".into(), 0),
                (Kind::Code, "fn x() {}".into(), 0),
                (Kind::Rule, "".into(), 0),
                (Kind::ListItem, "• [x] done".into(), 0),
            ]
        );
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
