//! The line editor's model: a note is a list of lines, each edited in its own
//! text field. These functions apply the edits that cross line boundaries and
//! say where the cursor goes next.

/// Where to put the cursor: a row and a byte offset inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub row: usize,
    pub col: usize,
}

pub fn split(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

pub fn join(lines: &[String]) -> String {
    lines.join("\n")
}

/// Markdown ATX heading: optional indentation, then `#`.
pub fn is_heading(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Row `i` now reads `text`, with the cursor at byte `cursor`. If the text
/// holds newlines (Enter or a paste), it becomes several rows, and the cursor
/// moves to wherever it ended up.
pub fn edit(lines: &mut Vec<String>, i: usize, text: &str, cursor: usize) -> Option<Focus> {
    if i >= lines.len() {
        return None;
    }
    if !text.contains('\n') {
        lines[i] = text.to_string();
        return None;
    }
    let cursor = floor_char_boundary(text, cursor);
    let before = &text[..cursor];
    let row = i + before.matches('\n').count();
    let col = before.rfind('\n').map_or(cursor, |p| cursor - p - 1);
    let parts = split(text);
    lines.splice(i..=i, parts);
    Some(Focus { row, col })
}

/// Backspace at the start of row `i`: append it to row `i - 1`.
pub fn join_up(lines: &mut Vec<String>, i: usize) -> Option<Focus> {
    if i == 0 || i >= lines.len() {
        return None;
    }
    let cur = lines.remove(i);
    let prev = &mut lines[i - 1];
    let col = prev.len();
    prev.push_str(&cur);
    Some(Focus { row: i - 1, col })
}

/// Delete at byte `cursor` in row `i`: at the end of the row, pull the next
/// row up into it. Otherwise it's an ordinary delete and nothing happens here.
pub fn delete_at(lines: &mut Vec<String>, i: usize, cursor: usize) -> Option<Focus> {
    if i + 1 >= lines.len() || cursor < lines[i].len() {
        return None;
    }
    let next = lines.remove(i + 1);
    let col = lines[i].len();
    lines[i].push_str(&next);
    Some(Focus { row: i, col })
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The smallest change that turns `old` into `new`: rows `start..old_end` of
/// `old` are replaced by rows `start..new_end` of `new`.
pub fn changed_range(old: &[String], new: &[String]) -> (usize, usize, usize) {
    let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - start;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    (start, old.len() - suffix, new.len() - suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn split_join_roundtrip() {
        for t in ["", "a", "a\n", "# T\n\nbody\n", "\n\n"] {
            assert_eq!(join(&split(t)), t);
        }
        assert_eq!(split(""), v(&[""]));
    }

    #[test]
    fn headings() {
        assert!(is_heading("# Title"));
        assert!(is_heading("  ## Sub"));
        assert!(!is_heading("text # not"));
        assert!(!is_heading(""));
    }

    #[test]
    fn enter_splits_and_moves_cursor() {
        let mut l = v(&["ab", "x"]);
        // Enter typed between a and b: the field now holds "a\nb", cursor after the newline.
        assert_eq!(edit(&mut l, 0, "a\nb", 2), Some(Focus { row: 1, col: 0 }));
        assert_eq!(l, v(&["a", "b", "x"]));
    }

    #[test]
    fn paste_of_several_lines() {
        let mut l = v(&["start end"]);
        let text = "start one\ntwo\nthree end";
        let cursor = "start one\ntwo\nthree".len();
        assert_eq!(edit(&mut l, 0, text, cursor), Some(Focus { row: 2, col: 5 }));
        assert_eq!(l, v(&["start one", "two", "three end"]));
    }

    #[test]
    fn plain_edit() {
        let mut l = v(&["a", "b"]);
        assert_eq!(edit(&mut l, 1, "bc", 2), None);
        assert_eq!(l, v(&["a", "bc"]));
    }

    #[test]
    fn backspace_and_delete_join() {
        let mut l = v(&["héllo", "world"]);
        assert_eq!(join_up(&mut l, 1), Some(Focus { row: 0, col: "héllo".len() }));
        assert_eq!(l, v(&["hélloworld"]));
        assert_eq!(join_up(&mut l, 0), None);

        let mut l = v(&["ab", "cd"]);
        assert_eq!(delete_at(&mut l, 0, 1), None);
        assert_eq!(delete_at(&mut l, 0, 2), Some(Focus { row: 0, col: 2 }));
        assert_eq!(l, v(&["abcd"]));
        assert_eq!(delete_at(&mut l, 0, 4), None);
    }

    #[test]
    fn changed_ranges() {
        let a = v(&["1", "2", "3"]);
        assert_eq!(changed_range(&a, &v(&["1", "x", "y", "3"])), (1, 2, 3));
        assert_eq!(changed_range(&a, &v(&["1", "3"])), (1, 2, 1));
        assert_eq!(changed_range(&a, &a), (3, 3, 3));
        assert_eq!(changed_range(&v(&["a", "a"]), &v(&["a", "a", "a"])), (2, 2, 3));
    }
}
