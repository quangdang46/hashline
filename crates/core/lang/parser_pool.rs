//! Thread-local cache of `tree_sitter::Parser` instances, keyed by [`Lang`].
//!
//! Constructing a `tree_sitter::Parser` and binding a grammar via
//! `set_language` is relatively expensive — for languages like Go the
//! grammar tables alone are several MB and the bind path runs through
//! tree-sitter's internal version checks. Profiling on this repo showed
//! `outline` at 53–150 ms for 1–10k lines on a fresh process, and the
//! parser construction dominated the cost for repeated calls within a
//! single process (e.g. `linehash mcp` serving multiple `outline` tools,
//! or batch tools that walk many files).
//!
//! Each thread owns a small `Vec<(Lang, Parser)>`. `with_parser` looks up
//! the existing parser by `Lang`, or constructs and inserts one on first
//! use. Because the parser is borrowed mutably through a closure we don't
//! have to worry about reentrant access on the same thread, and other
//! threads have their own pools.

use std::cell::RefCell;

use crate::lang::detect::Lang;

thread_local! {
    static PARSERS: RefCell<Vec<(Lang, tree_sitter::Parser)>> = const {
        RefCell::new(Vec::new())
    };
}

/// Map a [`Lang`] to its tree-sitter `Language`. Returns `None` for
/// languages we don't have a grammar for (plain text, unknown, etc.).
pub fn tree_sitter_language(lang: Lang) -> Option<tree_sitter::Language> {
    let ts = match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE,
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE,
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        Lang::Python => tree_sitter_python::LANGUAGE,
        Lang::Go => tree_sitter_go::LANGUAGE,
        Lang::C => tree_sitter_c::LANGUAGE,
        Lang::Cpp => tree_sitter_cpp::LANGUAGE,
        _ => return None,
    };
    Some(ts.into())
}

/// Run `f` with a tree-sitter `Parser` already bound to the grammar for
/// `lang`. Reuses a cached parser when one exists on this thread,
/// otherwise constructs and caches a fresh parser.
///
/// Returns `None` if `lang` has no grammar or if `set_language` fails
/// (e.g. ABI mismatch); the caller is expected to fall back to a
/// non-tree-sitter path in that case.
pub fn with_parser<F, R>(lang: Lang, f: F) -> Option<R>
where
    F: FnOnce(&mut tree_sitter::Parser) -> R,
{
    PARSERS.with(|cell| {
        // Fast path: parser already cached for this lang.
        let cached_idx = cell.borrow().iter().position(|(l, _)| *l == lang);
        if let Some(idx) = cached_idx {
            let mut parsers = cell.borrow_mut();
            return Some(f(&mut parsers[idx].1));
        }

        // Slow path: build a new parser, bind the grammar, cache it.
        let ts_lang = tree_sitter_language(lang)?;
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&ts_lang).is_err() {
            return None;
        }

        let mut parsers = cell.borrow_mut();
        parsers.push((lang, parser));
        let last = parsers.last_mut().expect("just pushed");
        Some(f(&mut last.1))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_same_parser_for_same_lang_on_same_thread() {
        let p1 = with_parser(Lang::Rust, |p| p as *const _ as usize).unwrap();
        let p2 = with_parser(Lang::Rust, |p| p as *const _ as usize).unwrap();
        assert_eq!(
            p1, p2,
            "parser should be cached across calls on the same thread"
        );
    }

    #[test]
    fn returns_none_for_unsupported_lang() {
        assert!(with_parser(Lang::PlainText, |_| ()).is_none());
        assert!(with_parser(Lang::Markdown, |_| ()).is_none());
        assert!(with_parser(Lang::Json, |_| ()).is_none());
    }

    #[test]
    fn caches_different_parsers_per_lang() {
        let rust = with_parser(Lang::Rust, |p| p as *const _ as usize).unwrap();
        let python = with_parser(Lang::Python, |p| p as *const _ as usize).unwrap();
        assert_ne!(rust, python);
    }

    #[test]
    fn parsers_parse_correctly_after_reuse() {
        // Call once to seed cache, then call again and verify the parser
        // still produces a usable tree.
        let _ = with_parser(Lang::Rust, |p| p.parse("fn a() {}", None));
        let tree = with_parser(Lang::Rust, |p| p.parse("fn b() {}", None))
            .expect("parser available")
            .expect("tree produced");
        assert!(!tree.root_node().has_error());
    }
}
