//! Line-level text editing that keeps everything outside the edited lines byte-for-byte.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines {
    pub lines: Vec<String>,
    pub eol: &'static str,
    /// Whether the original text ended with a line break.
    pub final_eol: bool,
}

pub fn split_lines(text: &str) -> Lines {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let final_eol = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect();
    if final_eol {
        lines.pop();
    }
    Lines {
        lines,
        eol,
        final_eol,
    }
}

pub fn join_lines(l: &Lines) -> String {
    if l.lines.is_empty() {
        return String::new();
    }
    let mut out = l.lines.join(l.eol);
    if l.final_eol {
        out.push_str(l.eol);
    }
    out
}

fn is_blank(s: Option<&String>) -> bool {
    s.is_some_and(|s| s.trim().is_empty())
}

/// FNV-1a over the entry's lines: detects that an entry changed since the caller read it.
pub fn fingerprint<S: AsRef<str>>(lines: &[S]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            h = (h ^ u32::from(b'\n')).wrapping_mul(0x0100_0193);
        }
        for byte in l.as_ref().trim_end().bytes() {
            h = (h ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
    }
    format!("{h:08x}")
}

/// The text lines of an entry, 1-based inclusive.
pub fn entry_lines(l: &Lines, line: u32, end_line: u32) -> &[String] {
    let len = l.lines.len();
    let from = (line as usize).saturating_sub(1).min(len);
    let to = (end_line as usize).clamp(from, len);
    &l.lines[from..to]
}

/// Replaces lines `[line, end_line]` (1-based, inclusive) with `block`.
pub fn replace_lines(l: &mut Lines, line: u32, end_line: u32, block: Vec<String>) {
    let len = l.lines.len();
    let from = (line as usize).saturating_sub(1).min(len);
    let to = (end_line as usize).clamp(from, len);
    l.lines.splice(from..to, block);
}

/// Removes lines `[line, end_line]` and returns how many lines went. When that leaves two blank
/// lines in a row (or a blank line at the start or end of the file), one of them goes too, so the
/// gap the entry occupied closes up.
pub fn remove_lines(l: &mut Lines, line: u32, end_line: u32) -> u32 {
    let i = (line as usize).saturating_sub(1).min(l.lines.len());
    let to = (end_line as usize).clamp(i, l.lines.len());
    l.lines.drain(i..to);
    let before = i.checked_sub(1).and_then(|j| l.lines.get(j));
    let after = l.lines.get(i);
    let mut extra = 0;
    if is_blank(after) && (i == 0 || is_blank(before)) {
        l.lines.remove(i);
        extra = 1;
    } else if after.is_none() && is_blank(before) {
        l.lines.remove(i - 1);
        extra = 1;
    }
    (end_line + 1).saturating_sub(line) + extra
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Separators {
    pub before: bool,
    pub after: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inserted {
    /// The block's first line, 1-based.
    pub start: u32,
    /// The first inserted line, separators included.
    pub at: u32,
    /// Lines inserted, separators included: what moves the lines below.
    pub count: u32,
}

/// Inserts `block` after line `after_line` (0 = top of file). A blank separator line is kept on a
/// side when either neighbour is a multi-line entry or the block itself is multi-line.
pub fn insert_block(
    l: &mut Lines,
    after_line: u32,
    block: Vec<String>,
    sep: Separators,
) -> Inserted {
    let mut at = after_line as usize;
    let mut lead = sep.before && at > 0 && !is_blank(l.lines.get(at - 1));
    if lead && is_blank(l.lines.get(at)) {
        // Reuse the existing blank line.
        at += 1;
        lead = false;
    }
    let next = l.lines.get(at);
    let trail = sep.after && next.is_some() && !is_blank(next);
    let count = block.len() as u32 + u32::from(lead) + u32::from(trail);
    let mut insert = Vec::with_capacity(count as usize);
    if lead {
        insert.push(String::new());
    }
    insert.extend(block);
    if trail {
        insert.push(String::new());
    }
    let pos = at.min(l.lines.len());
    l.lines.splice(pos..pos, insert);
    Inserted {
        start: at as u32 + if lead { 2 } else { 1 },
        at: at as u32 + 1,
        count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_trip() {
        for text in ["", "a", "a\n", "a\r\nb\r\n", "a\n\nb", "\n"] {
            assert_eq!(join_lines(&split_lines(text)), text, "{text:?}");
        }
        let l = split_lines("");
        assert!(l.lines.is_empty() && l.final_eol);
    }

    #[test]
    fn fingerprint_ignores_trailing_whitespace() {
        assert_eq!(fingerprint(&["a  ", "b"]), fingerprint(&["a", "b\t"]));
        assert_ne!(fingerprint(&["a", "b"]), fingerprint(&["ab"]));
        // FNV-1a of the empty string.
        assert_eq!(fingerprint::<&str>(&[]), "811c9dc5");
    }

    #[test]
    fn remove_closes_the_gap() {
        let mut l = split_lines("a\n\nb\nc\n\nd\n");
        assert_eq!(remove_lines(&mut l, 3, 4), 3);
        assert_eq!(join_lines(&l), "a\n\nd\n");
        let mut l = split_lines("a\n\nb\n");
        assert_eq!(remove_lines(&mut l, 3, 3), 2);
        assert_eq!(join_lines(&l), "a\n");
    }

    #[test]
    fn insert_reuses_a_blank_line() {
        let mut l = split_lines("a\n\nb\n");
        let r = insert_block(
            &mut l,
            1,
            vec!["x".into(), "y".into()],
            Separators {
                before: true,
                after: true,
            },
        );
        assert_eq!(join_lines(&l), "a\n\nx\ny\n\nb\n");
        assert_eq!(
            r,
            Inserted {
                start: 3,
                at: 3,
                count: 3
            }
        );
    }
}
