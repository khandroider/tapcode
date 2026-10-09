//! Whole-buffer highlighting via tree-sitter-highlight → sorted byte spans.
// ponytail: re-highlights the whole file on every edit; incremental parsing when files > ~1MB feel slow.

use super::languages::{Lang, by_name};
use tree_sitter_highlight::{HighlightEvent, Highlighter};

/// Recognized capture names. Theme lookup falls back by dotted prefix.
pub const NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "function",
    "function.builtin",
    "function.method",
    "keyword",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "text.title",
    "text.literal",
    "text.emphasis",
    "text.strong",
    "field",
    "function.call",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub name: usize,
}

pub fn highlight(lang: Lang, src: &[u8]) -> Vec<Span> {
    let Some(cfg) = lang.config() else { return Vec::new() };
    let mut hl = Highlighter::new();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    // Closure (not fn pointer): ties the 'static configs to the iterator's borrow lifetime.
    #[allow(clippy::redundant_closure)]
    let Ok(events) = hl.highlight(cfg, src, None, None, |name| by_name(name)) else {
        return out;
    };
    for ev in events.flatten() {
        match ev {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&name) = stack.last() {
                    out.push(Span { start, end, name });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_keyword_and_string_are_highlighted() {
        let src = b"<?php function hi() { return \"x\"; }";
        let spans = highlight(Lang::Php, src);
        let kw = NAMES.iter().position(|n| *n == "keyword").unwrap();
        let st = NAMES.iter().position(|n| *n == "string").unwrap();
        assert!(
            spans.iter().any(|s| s.name == kw && &src[s.start..s.end] == b"function"),
            "{spans:?}"
        );
        assert!(spans.iter().any(|s| s.name == st), "{spans:?}");
        assert!(spans.windows(2).all(|w| w[0].start <= w[1].start));
    }
}
