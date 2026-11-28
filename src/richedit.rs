//! The note editor's styled view: bold, italic and bigger headings while
//! editing the markdown source.
//!
//! Slint has no editable styled text, so the editor keeps an invisible
//! `TextInput` for typing (keys, input methods, undo, clipboard) and draws
//! the text itself from this layout: runs of styled text, the cursor and the
//! selection. The editor font is monospace, and its bold and italic faces
//! have the same advance, so every character sits on a grid: a column is
//! `adv` wide for the line's font size. That gives the x of any offset and
//! the offset under any point without measuring text.
//!
//! Offsets in and out of this module are byte offsets in the note's text.
//! While an input method composes a word (preedit), it's shown at the cursor,
//! underlined; the layout works on that "display" text and maps offsets.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Font size of each size class, relative to the editor font size:
/// 0 body (and headings 4-6), 1 heading 3, 2 heading 2, 3 heading 1.
pub const SCALE: [f32; 4] = [1.0, 1.15, 1.3, 1.5];

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
/// Markup characters (`**`, `_`, `#`): drawn fainter.
const DIM: u8 = 4;
/// Text an input method is composing: underlined.
const PREEDIT: u8 = 8;

/// The editor font, measured by the UI for each size class.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Font size.
    pub size: [f32; 4],
    /// Width of one column (one character).
    pub adv: [f32; 4],
    /// Height of one line.
    pub lh: [f32; 4],
}

/// A grapheme as placed on screen.
#[derive(Clone, Debug)]
struct Glyph {
    start: usize,
    end: usize,
    x: f32,
    w: f32,
    flags: u8,
    /// Callout colour (`markdown::CalloutColor::code`), 0 outside callouts.
    color: u8,
    /// Drawn on its own (not in a run): characters the editor font may not
    /// have (their fallback font's width differs), tabs and control chars.
    alone: bool,
    /// Tabs and control characters take space but draw nothing.
    blank: bool,
}

#[derive(Clone, Debug)]
struct Row {
    /// Display byte range, without the line's `\n`.
    start: usize,
    end: usize,
    y: f32,
    h: f32,
    class: usize,
    glyphs: Range<usize>,
    /// The last row of its line (not wrapped onto the next row).
    last: bool,
}

/// A piece of text to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub underline: bool,
    /// Callout colour (`markdown::CalloutColor::code`), 0 for the usual.
    pub color: i32,
    /// In the selection: drawn in the selection's text colour.
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Cursor moves that depend on the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    PageUp,
    PageDown,
    RowStart,
    RowEnd,
    /// One character (grapheme) back or forward.
    Left,
    Right,
}

impl Move {
    pub fn from_code(c: i32) -> Option<Move> {
        Some(match c {
            0 => Move::Up,
            1 => Move::Down,
            2 => Move::PageUp,
            3 => Move::PageDown,
            4 => Move::RowStart,
            5 => Move::RowEnd,
            6 => Move::Left,
            7 => Move::Right,
            _ => return None,
        })
    }
}

pub struct Layout {
    /// The text with the preedit inserted.
    display: String,
    /// Where the preedit is in `display`, and its length.
    pre_at: usize,
    pre_len: usize,
    text_len: usize,
    rows: Vec<Row>,
    glyphs: Vec<Glyph>,
    pub height: f32,
    metrics: Metrics,
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Size class of a heading level.
fn class_of(level: usize) -> usize {
    match level {
        1 => 3,
        2 => 2,
        3 => 1,
        _ => 0,
    }
}

/// Style flags for each byte of `text`, the heading level of each byte (0
/// outside headings), and its callout colour (0 outside callouts).
fn styles(text: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut flags = vec![0u8; text.len()];
    let mut level = vec![0u8; text.len()];
    let mut color = vec![0u8; text.len()];
    let bytes = text.as_bytes();
    let mark = |flags: &mut Vec<u8>, r: Range<usize>, f: u8| {
        for b in &mut flags[r] {
            *b |= f;
        }
    };
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for (ev, r) in Parser::new_ext(text, opts).into_offset_iter() {
        let r = r.start.min(text.len())..r.end.min(text.len());
        match ev {
            Event::Start(Tag::Heading { level: l, .. }) => {
                for b in &mut level[r.clone()] {
                    *b = l as u8;
                }
                mark(&mut flags, r.clone(), BOLD);
                // The `#`s of an ATX heading.
                let hashes = bytes[r.start..r.end].iter().take_while(|&&c| c == b'#').count();
                mark(&mut flags, r.start..r.start + hashes, DIM);
            }
            // A callout's first line: `> [!type]- Title`.
            Event::Start(Tag::BlockQuote(_)) => {
                let line_end = text[r.start..].find('\n').map_or(text.len(), |i| r.start + i);
                let line = &text[r.start..line_end];
                let Some(open) = line.find("[!") else { continue };
                if !line[..open].chars().all(|c| c == '>' || c == ' ')
                    || crate::markdown::parse_callout_line(&line[open..]).is_none()
                {
                    continue;
                }
                let Some((kind, _, _)) = crate::markdown::parse_callout_line(&line[open..]) else { continue };
                // The whole callout takes its colour (a nested one, its own).
                let c = crate::markdown::callout_color(&kind).code() as u8;
                for b in &mut color[r.clone()] {
                    *b = c;
                }
                let Some(close) = line[open..].find(']') else { continue };
                let mut end = r.start + open + close + 1;
                if matches!(bytes.get(end), Some(b'-' | b'+')) {
                    end += 1;
                }
                mark(&mut flags, r.start + open..end, DIM);
                mark(&mut flags, end..line_end, BOLD);
            }
            Event::Start(Tag::Strong) | Event::Start(Tag::Emphasis) => {
                let (f, n) = if matches!(ev, Event::Start(Tag::Strong)) { (BOLD, 2) } else { (ITALIC, 1) };
                mark(&mut flags, r.clone(), f);
                if r.len() >= 2 * n {
                    let is_delim = |i: usize| bytes[i] == b'*' || bytes[i] == b'_';
                    if (r.start..r.start + n).all(is_delim) && (r.end - n..r.end).all(is_delim) {
                        mark(&mut flags, r.start..r.start + n, DIM);
                        mark(&mut flags, r.end - n..r.end, DIM);
                    }
                }
            }
            _ => {}
        }
    }
    (flags, level, color)
}

impl Layout {
    /// Lay out `text` (with `preedit` shown at `cursor`) in `width`.
    pub fn new(text: &str, cursor: usize, preedit: &str, width: f32, m: &Metrics) -> Layout {
        let pre_at = if preedit.is_empty() { 0 } else { floor_char_boundary(text, cursor) };
        let display = if preedit.is_empty() {
            text.to_string()
        } else {
            let mut d = String::with_capacity(text.len() + preedit.len());
            d.push_str(&text[..pre_at]);
            d.push_str(preedit);
            d.push_str(&text[pre_at..]);
            d
        };
        let (mut flags, level, color) = styles(&display);
        for f in &mut flags[pre_at..pre_at + preedit.len()] {
            *f |= PREEDIT;
        }

        let mut rows = Vec::new();
        let mut glyphs = Vec::new();
        let mut y = 0f32;
        let mut line_start = 0;
        for line in display.split('\n') {
            let line_end = line_start + line.len();
            let class = class_of(level.get(line_start).copied().unwrap_or(0) as usize);
            let adv = m.adv[class].max(0.1);
            let lh = m.lh[class].max(1.0);
            let cols = ((width / adv) + 0.001).floor().max(1.0) as usize;

            // (start, end, columns, alone, blank, space)
            let mut items: Vec<(usize, usize, usize, bool, bool, bool)> = Vec::new();
            for (i, g) in line.grapheme_indices(true) {
                let start = line_start + i;
                let end = start + g.len();
                let ch = g.chars().next().unwrap_or(' ');
                if g == "\t" {
                    items.push((start, end, 0, true, true, true)); // width set while wrapping
                } else if g.chars().all(char::is_control) {
                    items.push((start, end, 0, true, true, false));
                } else {
                    let w = g.width().clamp(1, 2);
                    let alone = ch > '\u{24F}';
                    items.push((start, end, w, alone, false, ch == ' '));
                }
            }

            // Word wrap: break after the last space that fits; a word longer
            // than the row is broken anywhere. Spaces may hang past the edge.
            let mut row_first = 0; // index into items
            let mut col = 0usize;
            let mut brk: Option<usize> = None; // item index starting a new row
            let mut placed: Vec<(usize, usize)> = Vec::with_capacity(items.len()); // (col, width)
            let mut row_starts = vec![0usize];
            let mut i = 0;
            while i < items.len() {
                let (_, _, w0, _, blank, space) = items[i];
                let w = if blank && space { 4 - col % 4 } else { w0 };
                if !space && col > 0 && col + w > cols && i > row_first {
                    let at = match brk {
                        Some(b) if b > row_first => b,
                        _ => i,
                    };
                    row_first = at;
                    row_starts.push(at);
                    placed.truncate(at);
                    col = 0;
                    brk = None;
                    i = at;
                    continue;
                }
                placed.push((col, w));
                col += w;
                if space {
                    brk = Some(i + 1);
                }
                i += 1;
            }
            row_starts.push(items.len());

            let nrows = row_starts.len() - 1;
            for r in 0..nrows {
                let (a, b) = (row_starts[r], row_starts[r + 1]);
                let g0 = glyphs.len();
                let base = if a < b { placed[a].0 } else { 0 };
                for k in a..b {
                    let (s, e, _, alone, blank, _) = items[k];
                    let (c, w) = placed[k];
                    glyphs.push(Glyph {
                        start: s,
                        end: e,
                        x: (c - base) as f32 * adv,
                        w: w as f32 * adv,
                        flags: flags[s],
                        color: color[s],
                        alone,
                        blank,
                    });
                }
                let start = if a < items.len() { items[a].0 } else { line_end };
                let end = if b < items.len() && r + 1 < nrows { items[b].0 } else { line_end };
                rows.push(Row { start, end, y, h: lh, class, glyphs: g0..glyphs.len(), last: r + 1 == nrows });
                y += lh;
            }
            line_start = line_end + 1;
        }

        Layout {
            display,
            pre_at,
            pre_len: preedit.len(),
            text_len: text.len(),
            rows,
            glyphs,
            height: y,
            metrics: *m,
        }
    }

    /// The text (without the preedit) before a text offset.
    fn text_before(&self, off: usize) -> String {
        let d = self.to_display(off.min(self.text_len));
        let mut t = self.display[..floor_char_boundary(&self.display, d)].to_string();
        if self.pre_len > 0 && d >= self.pre_at + self.pre_len {
            t.replace_range(self.pre_at..self.pre_at + self.pre_len, "");
        }
        t
    }

    /// Text offset -> display offset.
    fn to_display(&self, off: usize) -> usize {
        let off = off.min(self.text_len);
        if self.pre_len > 0 && off >= self.pre_at {
            off + self.pre_len
        } else {
            off
        }
    }

    /// Display offset -> text offset.
    fn to_text(&self, off: usize) -> usize {
        if self.pre_len == 0 || off < self.pre_at {
            off
        } else if off < self.pre_at + self.pre_len {
            self.pre_at
        } else {
            off - self.pre_len
        }
    }

    fn row_of(&self, d: usize) -> usize {
        self.rows.partition_point(|r| r.start <= d).saturating_sub(1)
    }

    fn row_at_y(&self, y: f32) -> usize {
        self.rows.partition_point(|r| r.y <= y).saturating_sub(1)
    }

    fn row_glyphs(&self, r: &Row) -> &[Glyph] {
        &self.glyphs[r.glyphs.clone()]
    }

    /// x of a display offset in its row.
    fn x_in_row(&self, r: &Row, d: usize) -> f32 {
        let gs = self.row_glyphs(r);
        for g in gs {
            if g.start >= d {
                return g.x;
            }
        }
        gs.last().map_or(0.0, |g| g.x + g.w)
    }

    /// Where the cursor goes at the end of a row: before its last character
    /// when the row is wrapped (its end is the next row's start).
    fn row_end(&self, r: &Row) -> usize {
        match self.row_glyphs(r).last() {
            Some(g) if !r.last => g.start,
            _ => r.end,
        }
    }

    /// Display offset in row `ri` nearest to x.
    fn hit_row(&self, ri: usize, x: f32) -> usize {
        let r = &self.rows[ri];
        for g in self.row_glyphs(r) {
            if x < g.x + g.w / 2.0 {
                return g.start;
            }
        }
        self.row_end(r)
    }

    /// The cursor's rectangle (x, y, height) for a text offset.
    pub fn caret(&self, off: usize) -> Rect {
        let mut d = self.to_display(off);
        if self.pre_len > 0 && off == self.pre_at.min(self.text_len) {
            d = self.pre_at + self.pre_len;
        }
        let r = &self.rows[self.row_of(d)];
        Rect { x: self.x_in_row(r, d), y: r.y, w: 0.0, h: r.h }
    }

    /// Text offset under a point.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        if y < 0.0 {
            return 0;
        }
        if y >= self.height {
            return self.text_len;
        }
        self.to_text(self.hit_row(self.row_at_y(y), x))
    }

    /// Selection highlight rectangles between two text offsets.
    pub fn selection(&self, a: usize, b: usize) -> Vec<Rect> {
        let (s, e) = (self.to_display(a.min(b)), self.to_display(a.max(b)));
        if s == e {
            return Vec::new();
        }
        let mut out = Vec::new();
        for ri in self.row_of(s)..self.rows.len() {
            let r = &self.rows[ri];
            if r.start > e || (r.start == e && ri > self.row_of(s)) {
                break;
            }
            let x0 = self.x_in_row(r, s.max(r.start));
            let x1 = if e > r.end || (e == r.end && !r.last) {
                // Selected past the row: through its end, and a bit for the newline.
                let right = self.row_glyphs(r).last().map_or(0.0, |g| g.x + g.w);
                right + if r.last { self.metrics.adv[r.class] * 0.5 } else { 0.0 }
            } else {
                self.x_in_row(r, e)
            };
            if x1 > x0 {
                out.push(Rect { x: x0, y: r.y, w: x1 - x0, h: r.h });
            }
        }
        out
    }

    /// The word (or run of spaces) at a text offset, as a text range.
    pub fn word(&self, off: usize) -> (usize, usize) {
        let d = self.to_display(off);
        let r = &self.rows[self.row_of(d)];
        // The whole line, not just the row.
        let mut ls = r.start;
        while ls > 0 && self.display.as_bytes()[ls - 1] != b'\n' {
            ls -= 1;
        }
        let le = self.display[ls..].find('\n').map_or(self.display.len(), |i| ls + i);
        let line = &self.display[ls..le];
        for (i, w) in line.split_word_bound_indices() {
            let (s, e) = (ls + i, ls + i + w.len());
            if d >= s && d < e {
                return (self.to_text(s), self.to_text(e));
            }
        }
        (off, off)
    }

    /// Move the cursor from `off`. `goal` is the x the cursor keeps across
    /// up/down moves; returns the new offset and that x.
    pub fn move_cursor(&self, off: usize, mv: Move, goal: Option<f32>, page: f32) -> (usize, Option<f32>) {
        let d = self.to_display(off);
        let ri = self.row_of(d);
        let r = &self.rows[ri];
        let x = goal.unwrap_or_else(|| self.x_in_row(r, d));
        let last = self.rows.len() - 1;
        let to = |ri: usize| (self.to_text(self.hit_row(ri, x)), Some(x));
        match mv {
            Move::Up if ri == 0 => (0, None),
            Move::Up => to(ri - 1),
            Move::Down if ri == last => (self.text_len, None),
            Move::Down => to(ri + 1),
            Move::PageUp => {
                let y = r.y - page.max(r.h);
                if y < 0.0 {
                    (0, None)
                } else {
                    to(self.row_at_y(y))
                }
            }
            Move::PageDown => {
                let y = r.y + page.max(r.h);
                if y >= self.height {
                    (self.text_len, None)
                } else {
                    to(self.row_at_y(y))
                }
            }
            Move::RowStart => (self.to_text(r.start), None),
            Move::RowEnd => (self.to_text(self.row_end(r)), None),
            Move::Left => {
                let t = self.text_before(off);
                (t.grapheme_indices(true).next_back().map_or(0, |(i, _)| i), None)
            }
            Move::Right => {
                let off = floor_char_boundary(&self.display, self.to_display(off));
                let next = self.display[off..].graphemes(true).next().map_or(0, str::len);
                (self.to_text(off + next), None)
            }
        }
    }

    /// The runs to draw in rows between y `top` and `bottom`, with the
    /// selection between text offsets `sel`.
    pub fn runs(&self, top: f32, bottom: f32, sel: (usize, usize)) -> Vec<Run> {
        let (ss, se) = (self.to_display(sel.0.min(sel.1)), self.to_display(sel.0.max(sel.1)));
        let selected = |g: &Glyph| g.start >= ss && g.start < se;
        let mut out = Vec::new();
        let first = self.row_at_y(top.max(0.0));
        for r in &self.rows[first..] {
            if r.y > bottom {
                break;
            }
            let gs = self.row_glyphs(r);
            let mut i = 0;
            while i < gs.len() {
                let g = &gs[i];
                let mut j = i + 1;
                if !g.alone {
                    while j < gs.len() && !gs[j].alone && gs[j].flags == g.flags && gs[j].color == g.color && selected(&gs[j]) == selected(g) {
                        j += 1;
                    }
                }
                let last = &gs[j - 1];
                let text = &self.display[g.start..last.end];
                let underline = g.flags & PREEDIT != 0;
                if !g.blank && (underline || !text.trim().is_empty()) {
                    out.push(Run {
                        text: text.to_string(),
                        x: g.x,
                        y: r.y,
                        w: last.x + last.w - g.x,
                        h: r.h,
                        size: self.metrics.size[r.class],
                        bold: g.flags & BOLD != 0,
                        italic: g.flags & ITALIC != 0,
                        dim: g.flags & DIM != 0,
                        underline,
                        color: g.color as i32,
                        selected: selected(g),
                    });
                }
                i = j;
            }
        }
        out
    }
}

/// Keeps the last layout, so the UI's bindings can ask about it cheaply.
#[derive(Default)]
pub struct Engine {
    key: Option<(String, usize, String, u32, [u32; 12])>,
    pub layout: Option<Layout>,
    pub gen: i32,
    /// The x kept across up/down moves, and the offset it was for.
    goal: Option<(usize, f32)>,
}

impl Engine {
    pub fn layout(&mut self, text: &str, cursor: usize, preedit: &str, width: f32, m: &Metrics) -> &Layout {
        let mbits = {
            let mut b = [0u32; 12];
            for k in 0..4 {
                b[k] = m.size[k].to_bits();
                b[4 + k] = m.adv[k].to_bits();
                b[8 + k] = m.lh[k].to_bits();
            }
            b
        };
        let cursor = if preedit.is_empty() { 0 } else { cursor };
        let same = matches!(&self.key, Some((t, c, p, w, mb))
            if t == text && *c == cursor && p == preedit && *w == width.to_bits() && *mb == mbits);
        if !same || self.layout.is_none() {
            self.layout = Some(Layout::new(text, cursor, preedit, width, m));
            self.key = Some((text.to_string(), cursor, preedit.to_string(), width.to_bits(), mbits));
            self.gen = self.gen.wrapping_add(1);
        }
        self.layout.as_ref().unwrap()
    }

    pub fn move_cursor(&mut self, off: usize, mv: Move, page: f32) -> usize {
        let Some(l) = &self.layout else { return off };
        let goal = match (mv, self.goal) {
            (Move::Up | Move::Down | Move::PageUp | Move::PageDown, Some((o, x))) if o == off => Some(x),
            _ => None,
        };
        let (new, x) = l.move_cursor(off, mv, goal, page);
        self.goal = x.map(|x| (new, x));
        new
    }
}

/// Connect the UI's `RichEdit` global to a layout engine.
pub fn install(ui: &crate::AppWindow) {
    use crate::{RichEdit, RichLayout, RichMetrics, RichRange, RichRect, RichRun};
    use slint::{ComponentHandle, ModelRc, VecModel};
    use std::cell::RefCell;
    use std::rc::Rc;

    let engine = Rc::new(RefCell::new(Engine::default()));
    let g = ui.global::<RichEdit>();
    let rect = |r: Rect| RichRect { x: r.x, y: r.y, w: r.w, h: r.h };
    let off = |i: i32| i.max(0) as usize;

    let e = engine.clone();
    g.on_layout(move |text, cursor, preedit, width, m: RichMetrics| {
        let m = Metrics {
            size: [m.size0, m.size1, m.size2, m.size3],
            adv: [m.adv0, m.adv1, m.adv2, m.adv3],
            lh: [m.lh0, m.lh1, m.lh2, m.lh3],
        };
        let mut e = e.borrow_mut();
        let height = e.layout(&text, off(cursor), &preedit, width, &m).height;
        RichLayout { gen: e.gen, height }
    });
    let e = engine.clone();
    g.on_runs(move |_, top, bottom, anchor, cursor| {
        let e = e.borrow();
        let runs: Vec<RichRun> = e.layout.as_ref().map_or_else(Vec::new, |l| {
            l.runs(top, bottom, (off(anchor), off(cursor)))
                .into_iter()
                .map(|r| RichRun {
                    text: r.text.into(),
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    size: r.size,
                    bold: r.bold,
                    italic: r.italic,
                    dim: r.dim,
                    underline: r.underline,
                    color: r.color,
                    selected: r.selected,
                })
                .collect()
        });
        ModelRc::new(VecModel::from(runs))
    });
    let e = engine.clone();
    g.on_caret(move |_, cursor| e.borrow().layout.as_ref().map_or_else(RichRect::default, |l| rect(l.caret(off(cursor)))));
    let e = engine.clone();
    g.on_selection(move |_, a, c| {
        let v: Vec<RichRect> =
            e.borrow().layout.as_ref().map_or_else(Vec::new, |l| l.selection(off(a), off(c)).into_iter().map(rect).collect());
        ModelRc::new(VecModel::from(v))
    });
    let e = engine.clone();
    g.on_hit(move |_, x, y| e.borrow().layout.as_ref().map_or(0, |l| l.hit(x, y) as i32));
    let e = engine.clone();
    g.on_word(move |_, o| {
        let (start, end) = e.borrow().layout.as_ref().map_or((off(o), off(o)), |l| l.word(off(o)));
        RichRange { start: start as i32, end: end as i32 }
    });
    let e = engine;
    g.on_move(move |_, cursor, mv, page| match Move::from_code(mv) {
        Some(mv) => e.borrow_mut().move_cursor(off(cursor), mv, page) as i32,
        None => cursor,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Metrics {
        Metrics { size: [10.0, 11.5, 13.0, 15.0], adv: [6.0, 7.0, 8.0, 9.0], lh: [12.0, 14.0, 16.0, 18.0] }
    }

    fn texts(runs: &[Run]) -> Vec<(&str, bool, bool, bool)> {
        runs.iter().map(|r| (r.text.as_str(), r.bold, r.italic, r.dim)).collect()
    }

    #[test]
    fn bold_italic_and_markers() {
        let l = Layout::new("a **b** _c_ d", 0, "", 600.0, &m());
        let r = l.runs(0.0, 100.0, (0, 0));
        assert_eq!(
            texts(&r),
            vec![
                ("a ", false, false, false),
                ("**", true, false, true),
                ("b", true, false, false),
                ("**", true, false, true),
                (" ", false, false, false),
                ("_", false, true, true),
                ("c", false, true, false),
                ("_", false, true, true),
                (" d", false, false, false),
            ]
            .into_iter()
            .filter(|t| !t.0.trim().is_empty())
            .collect::<Vec<_>>()
        );
        // On the grid: "**" starts at column 2.
        assert_eq!(r[1].x, 12.0);
        assert_eq!(r[2].x, 24.0);
    }

    #[test]
    fn headings_are_bigger_and_bold() {
        let l = Layout::new("# T\n## U\n### V\nbody", 0, "", 600.0, &m());
        let r = l.runs(0.0, 1000.0, (0, 0));
        let find = |t: &str| r.iter().find(|x| x.text.trim() == t).unwrap().clone();
        assert_eq!(find("#").size, 15.0);
        assert!(find("#").dim);
        assert_eq!(find("T").size, 15.0);
        assert!(find("T").bold);
        assert_eq!(find("U").size, 13.0);
        assert_eq!(find("V").size, 11.5);
        assert_eq!(find("body").size, 10.0);
        assert!(!find("body").bold);
        // Rows stack with their own heights.
        assert_eq!(find("U").y, 18.0);
        assert_eq!(find("V").y, 34.0);
        assert_eq!(find("body").y, 48.0);
        assert_eq!(l.height, 60.0);
        // Heading text is placed on its bigger grid (" T" after "#").
        assert_eq!(find("T").x, 9.0);
    }

    #[test]
    fn callout_title_line() {
        let l = Layout::new("> [!tip]- Bona idea\n> text", 0, "", 600.0, &m());
        let r = l.runs(0.0, 100.0, (0, 0));
        let v: Vec<(&str, bool, bool, i32)> = r.iter().map(|r| (r.text.as_str(), r.bold, r.dim, r.color)).collect();
        // All of it green (tip), like the callout in the preview.
        assert_eq!(
            v,
            vec![("> ", false, false, 2), ("[!tip]-", false, true, 2), (" Bona idea", true, false, 2), ("> text", false, false, 2)]
        );
        // A plain quote keeps the usual colour.
        let l = Layout::new("> plain", 0, "", 600.0, &m());
        assert!(l.runs(0.0, 100.0, (0, 0)).iter().all(|r| r.color == 0));
    }

    #[test]
    fn no_emphasis_in_code() {
        let l = Layout::new("`a *b* c`\n```\n**x**\n```", 0, "", 600.0, &m());
        assert!(l.runs(0.0, 1000.0, (0, 0)).iter().all(|r| !r.bold && !r.italic));
    }

    #[test]
    fn caret_and_hit_roundtrip() {
        let t = "# Title\nhello world\n";
        let l = Layout::new(t, 0, "", 600.0, &m());
        // In the heading: column 3 at 9px per column.
        assert_eq!(l.caret(3), Rect { x: 27.0, y: 0.0, w: 0.0, h: 18.0 });
        // Second line.
        assert_eq!(l.caret(8 + 5), Rect { x: 30.0, y: 18.0, w: 0.0, h: 12.0 });
        for off in 0..=t.len() {
            let c = l.caret(off);
            assert_eq!(l.hit(c.x + 0.1, c.y + 1.0), off, "offset {off}");
        }
        // Past the end of a line: its end. Below everything: the end.
        assert_eq!(l.hit(500.0, 20.0), 8 + 11);
        assert_eq!(l.hit(5.0, 500.0), t.len());
        assert_eq!(l.hit(5.0, -3.0), 0);
    }

    #[test]
    fn word_wrap() {
        // 10 columns.
        let l = Layout::new("aaa bbb ccc dddddddddddddd", 0, "", 60.0, &m());
        let rows: Vec<&str> = l.rows.iter().map(|r| &l.display[r.start..r.end]).collect();
        assert_eq!(rows, vec!["aaa bbb ", "ccc ", "dddddddddd", "dddd"]);
        // The cursor at a wrap goes to the next row's start.
        assert_eq!(l.caret(8), Rect { x: 0.0, y: 12.0, w: 0.0, h: 12.0 });
        // End of a wrapped row: before its last character (the space).
        assert_eq!(l.move_cursor(1, Move::RowEnd, None, 0.0).0, 7);
        assert_eq!(l.move_cursor(9, Move::RowStart, None, 0.0).0, 8);
    }

    #[test]
    fn up_down_keep_column() {
        let t = "abcdef\n# H\nabcdef";
        let mut e = Engine::default();
        e.layout(t, 0, "", 600.0, &m());
        // From column 5 of line 1, down: the heading's grid is wider, "# H"
        // ends before x=30, so its end; then down again back to column 5.
        let a = e.move_cursor(5, Move::Down, 0.0);
        assert_eq!(a, 10);
        let b = e.move_cursor(a, Move::Down, 0.0);
        assert_eq!(b, 11 + 5);
        assert_eq!(e.move_cursor(b, Move::Up, 0.0), 10);
        assert_eq!(e.move_cursor(0, Move::Up, 0.0), 0);
        assert_eq!(e.move_cursor(12, Move::Down, 0.0), t.len());
    }

    #[test]
    fn left_right_by_grapheme() {
        let l = Layout::new("añ😀b", 0, "", 600.0, &m());
        assert_eq!(l.move_cursor(1, Move::Right, None, 0.0).0, 3);
        assert_eq!(l.move_cursor(3, Move::Right, None, 0.0).0, 7);
        assert_eq!(l.move_cursor(7, Move::Left, None, 0.0).0, 3);
        assert_eq!(l.move_cursor(0, Move::Left, None, 0.0).0, 0);
        assert_eq!(l.move_cursor(8, Move::Right, None, 0.0).0, 8);
    }

    #[test]
    fn selection_rects() {
        let l = Layout::new("ab\ncd", 0, "", 600.0, &m());
        let s = l.selection(1, 4);
        assert_eq!(s, vec![Rect { x: 6.0, y: 0.0, w: 9.0, h: 12.0 }, Rect { x: 0.0, y: 12.0, w: 6.0, h: 12.0 }]);
        assert!(l.selection(2, 2).is_empty());
    }

    #[test]
    fn preedit_shows_at_cursor() {
        // Composing "à" at offset 1 of "xy".
        let l = Layout::new("xy", 1, "à", 600.0, &m());
        let r = l.runs(0.0, 100.0, (0, 0));
        let u: Vec<&str> = r.iter().filter(|r| r.underline).map(|r| r.text.as_str()).collect();
        assert_eq!(u, vec!["à"]);
        // The cursor is after the preedit, "y" after it.
        assert_eq!(l.caret(1).x, 12.0);
        assert_eq!(l.caret(2).x, 18.0);
        assert_eq!(l.hit(13.0, 1.0), 1);
        assert_eq!(l.hit(16.0, 1.0), 2);
    }

    #[test]
    fn wide_and_non_latin_alone() {
        let l = Layout::new("a😀b", 0, "", 600.0, &m());
        let r = l.runs(0.0, 100.0, (0, 0));
        let v: Vec<(&str, f32)> = r.iter().map(|r| (r.text.as_str(), r.x)).collect();
        assert_eq!(v, vec![("a", 0.0), ("😀", 6.0), ("b", 18.0)]);
        // Accented Latin stays in the run.
        let l = Layout::new("pa amb tomàquet", 0, "", 600.0, &m());
        assert_eq!(l.runs(0.0, 100.0, (0, 0)).len(), 1);
    }

    #[test]
    fn tabs_and_cr() {
        let l = Layout::new("a\tb\r\nc", 0, "", 600.0, &m());
        assert_eq!(l.caret(2).x, 24.0);
        let r = l.runs(0.0, 100.0, (0, 0));
        assert!(r.iter().all(|r| !r.text.contains('\t') && !r.text.contains('\r')));
    }

    #[test]
    fn selected_runs_split() {
        let l = Layout::new("abcd **ef**", 0, "", 600.0, &m());
        let r = l.runs(0.0, 100.0, (8, 2));
        let v: Vec<(&str, bool)> = r.iter().map(|r| (r.text.as_str(), r.selected)).collect();
        assert_eq!(v, vec![("ab", false), ("cd ", true), ("**", true), ("e", true), ("f", false), ("**", false)]);
    }

    #[test]
    fn stale_offsets_are_clamped() {
        let l = Layout::new("añb", 0, "", 600.0, &m());
        l.caret(100);
        l.selection(0, 100);
        let _ = l.word(2);
        let l = Layout::new("", 0, "", 600.0, &m());
        assert_eq!(l.caret(0), Rect { x: 0.0, y: 0.0, w: 0.0, h: 12.0 });
        assert_eq!(l.hit(10.0, 5.0), 0);
        assert_eq!(l.move_cursor(0, Move::Down, None, 0.0).0, 0);
    }

    #[test]
    fn words() {
        let l = Layout::new("hola món, adéu", 0, "", 600.0, &m());
        assert_eq!(l.word(6), (5, 9));
        assert_eq!(l.word(1), (0, 4));
    }
}
