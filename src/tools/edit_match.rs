//! Finds the text an edit means to replace. Exact first, then progressively looser ways of
//! reading what the model sent (after opencode's replacer cascade and pi's unicode folding).
//! Content and needle are LF-only here; the edit tool handles line endings and BOMs.

use crate::limits;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Miss {
    NotFound { hint: Option<String> },
    Ambiguous(usize),
}

/// How a match relates to the text the model wrote, so the edit can fit its replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The span is the text as written.
    Span,
    /// The written text had whitespace around it that the span leaves out.
    Trimmed,
    /// Matched line by line with other indentation or spacing: re-indent the replacement.
    Lines,
}

struct Strategy {
    name: &'static str,
    find: fn(&Doc<'_>, &str) -> Vec<Span>,
    fit: Fit,
}

const fn strategy(name: &'static str, find: fn(&Doc<'_>, &str) -> Vec<Span>, fit: Fit) -> Strategy {
    Strategy { name, find, fit }
}

/// Tried in order; the first that finds anything decides.
const STRATEGIES: &[Strategy] = &[
    strategy("exact", exact, Fit::Span),
    strategy("trimmed boundary", trimmed_boundary, Fit::Trimmed),
    strategy("line trimmed", line_trimmed, Fit::Lines),
    strategy("whitespace normalized", whitespace_normalized, Fit::Lines),
    strategy("indentation flexible", indentation_flexible, Fit::Lines),
    strategy("escape normalized", escape_normalized, Fit::Span),
    strategy("unicode normalized", unicode_normalized, Fit::Span),
    strategy("line numbers stripped", line_numbers_stripped, Fit::Span),
    strategy(
        "line numbers stripped, trimmed",
        line_numbers_trimmed,
        Fit::Lines,
    ),
    strategy("block anchor", block_anchor, Fit::Span),
];

pub struct Found {
    pub spans: Vec<Span>,
    pub fit: Fit,
    /// Which strategy matched; "exact" unless the model's text had drifted.
    pub strategy: &'static str,
}

/// Where `needle` occurs in `content`: every occurrence when `all`, else exactly one. Without
/// `all`, two matches are ambiguous even when they overlap: either could be the one meant.
pub fn find(content: &str, needle: &str, all: bool) -> Result<Found, Miss> {
    let doc = Doc::new(content);
    for s in STRATEGIES {
        let mut spans = (s.find)(&doc, needle);
        spans.sort();
        spans.dedup();
        if all {
            spans = non_overlapping(spans);
        }
        let found = |spans| Found {
            spans,
            fit: s.fit,
            strategy: s.name,
        };
        match spans.len() {
            0 => continue,
            1 => return Ok(found(spans)),
            _ if all => return Ok(found(spans)),
            n => return Err(Miss::Ambiguous(n)),
        }
    }
    Err(Miss::NotFound {
        hint: closest(&doc, needle),
    })
}

struct Doc<'a> {
    text: &'a str,
    /// (start byte, line without its newline) for every line.
    lines: Vec<(usize, &'a str)>,
}

impl<'a> Doc<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split('\n') {
            lines.push((start, line));
            start += line.len() + 1;
        }
        Self { text, lines }
    }

    /// The span of lines `first..first + count`, plus the trailing newline when `newline`.
    fn span(&self, first: usize, count: usize, newline: bool) -> Span {
        let start = self.lines[first].0;
        let (last_start, last) = self.lines[first + count - 1];
        let mut end = last_start + last.len();
        if newline && end < self.text.len() {
            end += 1;
        }
        Span { start, end }
    }

    /// Every window of `lines.len()` lines where `same` holds line by line.
    fn windows(
        &self,
        lines: &[&str],
        same: impl Fn(&str, &str) -> bool,
        newline: bool,
    ) -> Vec<Span> {
        let n = lines.len();
        if n == 0 || n > self.lines.len() {
            return Vec::new();
        }
        (0..=self.lines.len() - n)
            .filter(|&i| (0..n).all(|k| same(self.lines[i + k].1, lines[k])))
            .map(|i| self.span(i, n, newline))
            .collect()
    }
}

/// Needle lines, without the empty piece a trailing newline leaves; and whether it had one.
fn needle_lines(needle: &str) -> (Vec<&str>, bool) {
    let newline = needle.ends_with('\n');
    let body = needle.strip_suffix('\n').unwrap_or(needle);
    (body.split('\n').collect(), newline)
}

/// Every occurrence, overlapping ones included (`aa` is in `aaa` twice).
fn exact(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut from = 0;
    while let Some(at) = doc.text[from..].find(needle) {
        let start = from + at;
        found.push(Span {
            start,
            end: start + needle.len(),
        });
        from = start + doc.text[start..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

fn trimmed_boundary(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let trimmed = needle.trim();
    if trimmed == needle || trimmed.is_empty() {
        return Vec::new();
    }
    exact(doc, trimmed)
}

fn line_trimmed(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let (lines, newline) = needle_lines(needle);
    doc.windows(&lines, |a, b| a.trim() == b.trim(), newline)
}

fn whitespace_normalized(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let (lines, newline) = needle_lines(needle);
    doc.windows(&lines, |a, b| squash(a) == squash(b), newline)
}

fn indentation_flexible(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let (lines, newline) = needle_lines(needle);
    let n = lines.len();
    if n == 0 || n > doc.lines.len() {
        return Vec::new();
    }
    let dedent = |ls: &[&str]| -> Vec<String> {
        let indent = ls
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        ls.iter()
            .map(|l| l.get(indent..).unwrap_or(l.trim_start()).to_owned())
            .collect()
    };
    let wanted = dedent(&lines);
    (0..=doc.lines.len() - n)
        .filter(|&i| {
            let window: Vec<&str> = doc.lines[i..i + n].iter().map(|(_, l)| *l).collect();
            dedent(&window) == wanted
        })
        .map(|i| doc.span(i, n, newline))
        .collect()
}

/// Models sometimes send `\n` and `\"` as literal backslash sequences.
fn escape_normalized(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    if !needle.contains('\\') {
        return Vec::new();
    }
    let unescaped = unescape(needle);
    if unescaped == needle {
        return Vec::new();
    }
    exact(doc, &unescaped)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c @ ('"' | '\'' | '\\' | '`' | '$')) => out.push(c),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Smart quotes, dashes and odd spaces, folded one character for one character.
fn fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
        other => other,
    }
}

fn unicode_normalized(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let folded_needle: String = needle.chars().map(fold).collect();
    let mut folded = String::with_capacity(doc.text.len());
    // Byte offset in `folded` -> byte offset in the original, for every char boundary.
    let mut back = Vec::with_capacity(doc.text.len() + 1);
    for (orig, c) in doc.text.char_indices() {
        let f = fold(c);
        for _ in 0..f.len_utf8() {
            back.push(orig);
        }
        folded.push(f);
    }
    back.push(doc.text.len());
    if folded == doc.text && folded_needle == needle {
        return Vec::new();
    }
    exact(&Doc::new(&folded), &folded_needle)
        .into_iter()
        .map(|s| Span {
            start: back[s.start],
            end: back[s.end],
        })
        .collect()
}

/// Text copied from `read` output still carrying its `   12\t` prefixes.
fn line_numbers_stripped(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    without_line_numbers(needle).map_or_else(Vec::new, |n| exact(doc, &n))
}

/// The same, where the copied lines also drifted in indentation.
fn line_numbers_trimmed(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    without_line_numbers(needle).map_or_else(Vec::new, |n| line_trimmed(doc, &n))
}

fn without_line_numbers(needle: &str) -> Option<String> {
    let (lines, newline) = needle_lines(needle);
    let stripped: Option<Vec<&str>> = lines
        .iter()
        .map(|l| {
            let rest = l.trim_start();
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            (digits > 0)
                .then(|| {
                    rest[digits..]
                        .strip_prefix('\t')
                        .or_else(|| rest[digits..].strip_prefix('→'))
                })
                .flatten()
        })
        .collect();
    let mut joined = stripped?.join("\n");
    if newline {
        joined.push('\n');
    }
    Some(joined)
}

/// First and last lines match exactly (trimmed); the middle only has to be similar.
fn block_anchor(doc: &Doc<'_>, needle: &str) -> Vec<Span> {
    let limits = &limits::get().edit;
    let (lines, newline) = needle_lines(needle);
    let n = lines.len();
    let (first, last) = (lines[0].trim(), lines[n - 1].trim());
    if n < 3 || first.is_empty() || last.is_empty() {
        return Vec::new();
    }
    let tolerance = (n * limits.size_tolerance_pct.value as usize / 100).max(1);
    let (shortest, longest) = (n.saturating_sub(tolerance).max(3), n + tolerance);
    let middle = lines[1..n - 1].join("\n");
    let threshold = limits.anchor_similarity_pct.value as f64 / 100.0;
    let mut found = Vec::new();
    for i in 0..doc.lines.len() {
        if doc.lines[i].1.trim() != first {
            continue;
        }
        let mut here = Vec::new();
        for len in shortest..=longest {
            let j = i + len - 1;
            if j >= doc.lines.len() {
                break;
            }
            if doc.lines[j].1.trim() != last {
                continue;
            }
            let window: Vec<&str> = doc.lines[i + 1..j].iter().map(|(_, l)| *l).collect();
            if similarity(&window.join("\n"), &middle) >= threshold {
                here.push((len, doc.span(i, len, newline)));
            }
        }
        // A block as long as the one written is the one meant, not a shorter one ending at an
        // inner closing line.
        match here.iter().find(|(len, _)| *len == n) {
            Some(&(_, span)) => found.push(span),
            None => found.extend(here.into_iter().map(|(_, span)| span)),
        }
    }
    found
}

/// The line most like the needle's first line, to point the model at the right place.
fn closest(doc: &Doc<'_>, needle: &str) -> Option<String> {
    let first = needle.lines().find(|l| !l.trim().is_empty())?.trim();
    if first.len() * doc.lines.len() > limits::get().edit.similarity_max_chars.value * 100 {
        return None;
    }
    let (index, score) = doc
        .lines
        .iter()
        .enumerate()
        .map(|(i, (_, l))| (i, similarity(l.trim(), first)))
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    (score >= 0.5).then(|| {
        format!(
            "The closest line is {} ({:.0}% similar): `{}`",
            index + 1,
            score * 100.0,
            doc.lines[index].1.trim()
        )
    })
}

/// Leading whitespace of the first non-blank line.
pub fn indent_of(text: &str) -> &str {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    &line[..line.len() - line.trim_start().len()]
}

/// 1.0 for equal strings, falling towards 0.0 with edit distance.
fn similarity(a: &str, b: &str) -> f64 {
    let max_chars = limits::get().edit.similarity_max_chars.value;
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    if a.len() > max_chars || b.len() > max_chars {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    1.0 - prev[b.len()] as f64 / longest as f64
}

fn non_overlapping(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if out.last().is_none_or(|prev| span.start >= prev.end) {
            out.push(span);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(content: &str, needle: &str) -> Option<&'static str> {
        let doc = Doc::new(content);
        STRATEGIES.iter().find_map(|s| {
            let mut spans = (s.find)(&doc, needle);
            spans.dedup();
            (spans.len() == 1).then_some(s.name)
        })
    }

    #[test]
    fn each_strategy_catches_its_kind_of_drift() {
        let code = "fn main() {\n    let x = \"hi\";\n    println!(\"{x}\");\n}\n";
        assert_eq!(one(code, "let x = \"hi\";"), Some("exact"));
        assert_eq!(one(code, "  let x = \"hi\";  \n"), Some("trimmed boundary"));
        assert_eq!(
            one(code, "let x = \"hi\";\nprintln!(\"{x}\");"),
            Some("line trimmed")
        );
        assert_eq!(
            one(code, "    let  x =  \"hi\";"),
            Some("whitespace normalized")
        );
        assert_eq!(one(code, "let x = \\\"hi\\\";"), Some("escape normalized"));
        assert_eq!(
            one(code, "let x = \u{201c}hi\u{201d};"),
            Some("unicode normalized")
        );
        assert_eq!(
            one(
                code,
                "     2\t    let x = \"hi\";\n     3\t    println!(\"{x}\");"
            ),
            Some("line numbers stripped")
        );
    }

    #[test]
    fn unicode_spans_map_back_to_original_bytes() {
        let content = "say \u{201c}hi\u{201d} now";
        let spans = find(content, "\"hi\"", false).unwrap().spans;
        assert_eq!(&content[spans[0].start..spans[0].end], "\u{201c}hi\u{201d}");
    }
}
