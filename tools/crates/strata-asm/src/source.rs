//! Physical-line handling: comment removal, statement splitting on `;`, and
//! C preprocessor line markers.

/// One statement cut out of a physical line: its text (comments blanked) and
/// the 0-based byte column where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub col: u32,
    pub text: String,
}

/// Splits physical lines into statements, carrying `/* ... */` comment state
/// from one line to the next.
#[derive(Debug, Default, Clone)]
pub struct LineSplitter {
    in_block_comment: bool,
}

impl LineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while inside an unterminated `/* ... */` comment.
    pub fn in_block_comment(&self) -> bool {
        self.in_block_comment
    }

    /// Splits `line` into statements. Comments are replaced by spaces so that
    /// byte columns stay valid; `#` comments run to the end of the line.
    /// Empty statements are dropped.
    pub fn split(&mut self, line: &str) -> Vec<Piece> {
        let bytes = line.as_bytes();
        let mut clean = bytes.to_vec();
        let mut i = 0;
        let mut cuts = Vec::new();
        while i < bytes.len() {
            if self.in_block_comment {
                if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    clean[i] = b' ';
                    clean[i + 1] = b' ';
                    i += 2;
                    self.in_block_comment = false;
                } else {
                    clean[i] = b' ';
                    i += 1;
                }
                continue;
            }
            match bytes[i] {
                b'"' => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                b'\'' => {
                    // A character literal: 'c', '\c' (closing quote optional).
                    i += 1;
                    if i < bytes.len() {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    if bytes.get(i) == Some(&b'\'') {
                        i += 1;
                    }
                }
                b'#' => {
                    for c in &mut clean[i..] {
                        *c = b' ';
                    }
                    break;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    clean[i] = b' ';
                    clean[i + 1] = b' ';
                    i += 2;
                    self.in_block_comment = true;
                }
                b';' => {
                    cuts.push(i);
                    i += 1;
                }
                _ => i += 1,
            }
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        for end in cuts.into_iter().chain(std::iter::once(clean.len())) {
            let seg = &clean[start..end.min(clean.len())];
            // Blanking only replaced ASCII bytes with ASCII spaces, and cuts are at
            // ASCII ';', so every segment is still valid UTF-8.
            let text = String::from_utf8_lossy(seg);
            let trimmed_start = text.len() - text.trim_start().len();
            let t = text.trim();
            if !t.is_empty() {
                pieces.push(Piece {
                    col: (start + trimmed_start) as u32,
                    text: t.to_string(),
                });
            }
            start = end + 1;
        }
        pieces
    }
}

/// Parses a C preprocessor line marker `# <number> "<file>" [flags...]`.
///
/// Returns the line number of the *next* line and the file name.
pub fn parse_line_marker(line: &str) -> Option<(u32, String)> {
    let rest = line.trim_start().strip_prefix('#')?;
    let rest = rest.strip_prefix("line").unwrap_or(rest);
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number: u32 = rest[..digits].parse().ok()?;
    let rest = &rest[digits..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix('"')?;
    let mut name = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => name.push(chars.next()?),
            c => name.push(c),
        }
    }
    // Only flags (digits) may follow.
    if !chars
        .as_str()
        .split_whitespace()
        .all(|f| f.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((number, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(lines: &[&str]) -> Vec<Vec<(u32, String)>> {
        let mut s = LineSplitter::new();
        lines
            .iter()
            .map(|l| s.split(l).into_iter().map(|p| (p.col, p.text)).collect())
            .collect()
    }

    #[test]
    fn comments_and_semicolons() {
        assert_eq!(
            split(&["  addi a0, a0, 1 # comment; not split"]),
            vec![vec![(2, "addi a0, a0, 1".into())]]
        );
        assert_eq!(
            split(&["a: nop; nop ;; li a0, ';'"]),
            vec![vec![
                (0, "a: nop".into()),
                (8, "nop".into()),
                (15, "li a0, ';'".into())
            ]]
        );
        assert_eq!(
            split(&[".ascii \"a#b;c\" # x"]),
            vec![vec![(0, ".ascii \"a#b;c\"".into())]]
        );
        assert_eq!(
            split(&[".byte '#', 1"]),
            vec![vec![(0, ".byte '#', 1".into())]]
        );
        assert_eq!(
            split(&[".ascii \"q\\\"#\""]),
            vec![vec![(0, ".ascii \"q\\\"#\"".into())]]
        );
    }

    #[test]
    fn block_comments_span_lines() {
        let out = split(&[
            "nop /* start",
            "still comment ; x",
            "end */ addi a0, a0, 1",
            "/**/ret",
        ]);
        assert_eq!(out[0], vec![(0, "nop".into())]);
        assert!(out[1].is_empty());
        assert_eq!(out[2], vec![(7, "addi a0, a0, 1".into())]);
        assert_eq!(out[3], vec![(4, "ret".into())]);
    }

    #[test]
    fn line_markers() {
        assert_eq!(
            parse_line_marker("# 12 \"foo.S\""),
            Some((12, "foo.S".into()))
        );
        assert_eq!(
            parse_line_marker("# 1 \"<built-in>\" 1 3"),
            Some((1, "<built-in>".into()))
        );
        assert_eq!(
            parse_line_marker("#line 7 \"a\\\\b.c\""),
            Some((7, "a\\b.c".into()))
        );
        assert_eq!(parse_line_marker("# comment"), None);
        assert_eq!(parse_line_marker("# 12 apples"), None);
        assert_eq!(parse_line_marker("# 12 \"x\" then words"), None);
        assert_eq!(parse_line_marker("#12 \"x\""), None);
        assert_eq!(parse_line_marker("nop # 12 \"x\""), None);
    }
}
