//! Built-in heuristic [`BlockResolver`] implementation — the single canonical
//! block-resolution engine for the whole crate.
//!
//! Used by:
//! - the library [`Editor`](crate::editor::Editor) via [`BlockResolver`],
//! - the CLI `patch` command's `SWAP.BLK`/`DEL.BLK`/`INS.BLK.*` ops (through
//!   [`resolve_span`]), and
//! - the CLI `find-block` command (through [`resolve_span`] /
//!   [`resolve_block_boundaries`]).
//!
//! Resolution strategy per language family:
//! - **Brace languages** (Rust, JS/TS, Go, C-family, …): a lexical scanner
//!   tracks strings, char literals, comments, regex literals, template
//!   literals, raw strings and heredocs, then balances `{`/`}`. Pairs joined
//!   by a same-line `} … {` bridge (`} else {`, `} catch {`, …) are merged so
//!   if/else chains resolve as one block.
//! - **Indent languages** (Python, YAML, Verse): backward scan to the
//!   enclosing header, forward scan to the dedent boundary. Body anchors
//!   resolve to the innermost enclosing suite, and `:`-terminated compound
//!   headers (`if x:`, `else:` continuations) are handled.
//! - **Ruby**: `def`/`class`/`if … end` keyword balancing.
//! - **Keyword languages** (CMake, PHP alternative syntax, Svelte `{#if}`):
//!   `end*`-keyword / `{/…}` balancing, tried where brace matching cannot see
//!   the construct.
//!
//! Everything is best-effort: when a construct cannot be resolved the
//! resolver returns `None` so callers fail loudly (`BLOCK_UNRESOLVED` /
//! `UnbalancedBlock`) instead of producing a wrong span.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use crate::document::LineEntry;
use crate::error::HashlineError;
use crate::types::{BlockResolver, BlockResolverRequest, BlockSpan};

/// Default heuristic block resolver.
///
/// This is the canonical resolver: the CLI patch/find-block paths and the
/// library Editor all resolve blocks through the same code in this module.
pub struct BuiltinBlockResolver;

impl BuiltinBlockResolver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BuiltinBlockResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockResolver for BuiltinBlockResolver {
    fn resolve(&self, request: &BlockResolverRequest) -> Option<BlockSpan> {
        let entries = text_to_entries(&request.text);
        let anchor_idx = request.line.wrapping_sub(1);
        let path = Path::new(&request.path);
        resolve_span(&entries, anchor_idx, path).map(|(start, end)| BlockSpan {
            start: start + 1,
            end: end + 1,
        })
    }
}

/// Convert text to line entries (for resolver use).
fn text_to_entries(text: &str) -> Vec<LineEntry> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|s| LineEntry {
            content: s.to_string(),
            short_hash: crate::hash::short_hash_value(s),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Canonical resolution entry point
// ---------------------------------------------------------------------------

/// Resolve the syntactic block containing `anchor_index` (0-based) in
/// `entries` for the language inferred from `path`.
///
/// Returns a 0-based inclusive `(start, end)` line span, or `None` when no
/// block can be resolved (caller must fail loudly). The returned span has
/// been extended upward over signature-continuation lines and doc-comment /
/// attribute / decorator lines so that e.g. `INS.BLK.PRE` does not detach a
/// doc comment from its item.
pub(crate) fn resolve_span(
    entries: &[LineEntry],
    anchor_index: usize,
    path: &Path,
) -> Option<(usize, usize)> {
    if anchor_index >= entries.len() {
        return None;
    }
    // Drop the phantom trailing empty entry (`split('\n')` artifact on
    // newline-terminated files) so no family can resolve a span that ends
    // on — and thereby deletes — the file's final newline. Only truncate
    // when the anchor itself is a real line; a phantom anchor keeps the
    // full slice so indexing stays in bounds and resolution fails loudly.
    let n = scan_len(entries);
    let entries = if anchor_index < n {
        &entries[..n]
    } else {
        entries
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let spec = spec_for(ext, path);

    // The bool marks whether signature-continuation extension is safe:
    // brace/indent spans can start on a multi-line signature tail (`) {`,
    // `):`), but keyword-delimited spans (CMake `if(…)`, Svelte `{#if}`)
    // legitimately end their header line with `)`/`>` and must not be
    // walked upward into an enclosing construct.
    let (mut span, sig_extendable) = match family_for(ext) {
        Family::Brace => brace_resolve(entries, anchor_index, &spec).map(|s| (s, true)),
        Family::Indent => indent_resolve(entries, anchor_index, &spec)
            .or_else(|| brace_resolve(entries, anchor_index, &spec))
            .map(|s| (s, true)),
        Family::Ruby => ruby_resolve(entries, anchor_index).map(|s| (s, true)),
        Family::KeywordFirst => keyword_resolve(entries, anchor_index, ext)
            .map(|s| (s, false))
            .or_else(|| brace_resolve(entries, anchor_index, &spec).map(|s| (s, true)))
            .or_else(|| indent_resolve(entries, anchor_index, &spec).map(|s| (s, true))),
        Family::BraceThenKeyword => brace_resolve(entries, anchor_index, &spec)
            .map(|s| (s, true))
            .or_else(|| keyword_resolve(entries, anchor_index, ext).map(|s| (s, false)))
            .or_else(|| indent_resolve(entries, anchor_index, &spec).map(|s| (s, true))),
        Family::Default => brace_resolve(entries, anchor_index, &spec)
            .map(|s| (s, true))
            .or_else(|| keyword_resolve(entries, anchor_index, ext).map(|s| (s, false)))
            .or_else(|| indent_resolve(entries, anchor_index, &spec).map(|s| (s, true))),
    }?;

    // Extend the span start upward over signature-continuation lines
    // (multi-line `fn f(\n … \n) {`) and doc-comment/attribute/decorator
    // lines, to a fixpoint (doc lines can themselves end in trigger chars).
    // Skip entirely when the resolved start is itself a blank/comment-only
    // line (e.g. a comment anchor that resolved to a single-line span) —
    // extending upward there would swallow the enclosing header.
    let start_has_code = {
        let t = entries[span.0].content.trim();
        !t.is_empty() && !code_part(t, &spec).trim().is_empty()
    };
    if start_has_code {
        if sig_extendable {
            loop {
                let before = span.0;
                span.0 = extend_signature_up(entries, span.0, &spec);
                span.0 = extend_doc_up(entries, span.0, &spec);
                if span.0 == before {
                    break;
                }
            }
        } else {
            // Keyword spans: doc comments still belong to the item, but
            // signature walking is unsafe (see above).
            span.0 = extend_doc_up(entries, span.0, &spec);
        }
    }
    Some(span)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `{`/`}` balanced languages.
    Brace,
    /// Indentation-based languages (Python, YAML, Verse).
    Indent,
    /// Ruby `end`-keyword languages.
    Ruby,
    /// Keyword-delimited constructs that must win over brace pairs
    /// (Svelte `{#if}…{/if}` — the `{`/`}` are balanced per line).
    KeywordFirst,
    /// Brace languages that also have keyword-delimited alternative syntax
    /// (CMake, PHP `if():…endif;`).
    BraceThenKeyword,
    /// Unknown: brace, then keyword, then indent.
    Default,
}

fn family_for(ext: &str) -> Family {
    match ext {
        "rs" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "go" | "java" | "c" | "cpp" | "cc"
        | "cxx" | "h" | "hh" | "hpp" | "hxx" | "cs" | "kt" | "kts" | "swift" | "scala" | "dart"
        | "zig" | "m" | "mm" | "groovy" | "gradle" | "sol" | "proto" | "ino" | "pde" | "cu"
        | "cuh" => Family::Brace,
        "py" | "verse" | "yaml" | "yml" => Family::Indent,
        "rb" => Family::Ruby,
        "svelte" => Family::KeywordFirst,
        "cmake" | "php" => Family::BraceThenKeyword,
        _ => Family::Default,
    }
}

// ---------------------------------------------------------------------------
// Per-language lexical spec
// ---------------------------------------------------------------------------

/// How `'` is interpreted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SqMode {
    /// No single-quote strings (should not happen for brace languages, but
    /// kept for completeness).
    Off,
    /// `'` opens a string only when a closing `'` exists later on the same
    /// line. Handles JS/C-family strings AND rejects apostrophes in JSX/TSX
    /// text (`Don't`) and Rust lifetimes (`'a`, `'static`).
    Smart,
    /// Rust char-literal aware: `'` opens a string only for `'x'`, `'\n'`,
    /// `b'x'` style literals — never for `'a` lifetimes.
    RustChar,
}

#[derive(Clone, Copy)]
struct Spec {
    /// Line-comment prefixes recognized at any code position.
    line_comments: &'static [&'static str],
    /// Block-comment `(start, end)` pairs.
    block_comments: &'static [(&'static str, &'static str)],
    sq: SqMode,
    /// `"` starts a string.
    dq: bool,
    /// `"` strings may span multiple lines (Rust, Zig, Kotlin `"""`, shell).
    persistent_dq: bool,
    /// `` ` `` starts a template/raw string (JS template literals, Go raw).
    backtick: bool,
    /// `/…/ ` regex literals (JS/TS family only).
    regex: bool,
    /// Rust raw strings `r"…"`, `r#"…"#`, `br#"…"#`.
    raw_string: bool,
    /// Python/Ruby triple-quoted strings `'''…'''` / `"""…"""`.
    triple_quote: bool,
    /// Shell-style heredocs `<<EOF` / `<<-'EOF'` / `<<~RUBY`.
    heredoc: bool,
    /// PowerShell here-strings `@"…"@` / `@'…'@`.
    ps_here_string: bool,
    /// Zig multiline strings: lines starting with `\\` are string content.
    zig_ml_string: bool,
    /// Comments recognized only at the start of a (trimmed) line.
    line_start_comments: &'static [&'static str],
    /// Indent language: a line whose code ends with `:` is a block header.
    indent_colon_header: bool,
    /// Indent language: `else:`/`elif`/`except`/`finally` at the same indent
    /// continue the block instead of ending it.
    indent_continuations: bool,
    /// Extra prefixes (beyond line comments) treated as doc lines for the
    /// upward doc/signature extension (`@` decorators/annotations, `*` JSDoc).
    doc_extra: &'static [&'static str],
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            line_comments: &["//"],
            block_comments: &[("/*", "*/")],
            sq: SqMode::Smart,
            dq: true,
            persistent_dq: false,
            backtick: false,
            regex: false,
            raw_string: false,
            triple_quote: false,
            heredoc: false,
            ps_here_string: false,
            zig_ml_string: false,
            line_start_comments: &[],
            indent_colon_header: false,
            indent_continuations: false,
            doc_extra: &[],
        }
    }
}

fn spec_for(ext: &str, path: &Path) -> Spec {
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let dockerfile = fname == "dockerfile" || fname.starts_with("dockerfile.");
    let d = Spec::default();
    match ext {
        "rs" => Spec {
            sq: SqMode::RustChar,
            persistent_dq: true,
            raw_string: true,
            doc_extra: &["#["],
            ..d
        },
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Spec {
            backtick: true,
            regex: true,
            doc_extra: &["@", "/*", "*/", "* "],
            ..d
        },
        "go" => Spec {
            backtick: true,
            doc_extra: &["@"],
            ..d
        },
        "zig" => Spec {
            persistent_dq: true,
            zig_ml_string: true,
            ..d
        },
        "kt" | "kts" | "swift" | "scala" | "dart" => Spec {
            persistent_dq: true,
            triple_quote: true, // Kotlin/Swift/Scala """, Dart """ and '''
            doc_extra: &["@"],
            ..d
        },
        "java" => Spec {
            persistent_dq: true,
            triple_quote: true, // Java text blocks """
            doc_extra: &["@", "/*", "*/", "* "],
            ..d
        },
        "cs" => Spec {
            persistent_dq: true,
            triple_quote: true, // C# 11 raw string literals """
            doc_extra: &["/*", "*/", "* "],
            ..d
        },
        "py" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            indent_colon_header: true,
            indent_continuations: true,
            triple_quote: true,
            doc_extra: &["@"],
            ..d
        },
        "verse" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            indent_colon_header: true,
            ..d
        },
        "yaml" | "yml" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            indent_colon_header: true,
            ..d
        },
        "rb" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            triple_quote: true,
            heredoc: true,
            ..d
        },
        "pl" | "pm" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            heredoc: true,
            ..d
        },
        "sh" | "bash" | "zsh" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            heredoc: true,
            persistent_dq: true,
            ..d
        },
        "fish" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            ..d
        },
        "ps1" | "psm1" | "psd1" => Spec {
            line_comments: &["#"],
            ps_here_string: true,
            ..d
        },
        "r" | "jl" | "nim" | "cr" | "ex" | "exs" | "coffee" | "tf" | "tfvars" | "graphql"
        | "gql" | "toml" | "cmake" | "mk" | "mak" | "nix" | "dhall" | "purs" => Spec {
            line_comments: &["#"],
            block_comments: &[],
            ..d
        },
        _ if dockerfile => Spec {
            line_comments: &["#"],
            block_comments: &[],
            heredoc: true,
            ..d
        },
        "sql" | "sqlite" | "lua" | "hs" | "lhs" | "ada" | "ads" | "elm" | "vhdl" | "abnf"
        | "proto" => Spec {
            line_comments: &["--"],
            block_comments: if ext == "lua" {
                &[("--[[", "]]")]
            } else if ext == "hs" || ext == "lhs" {
                &[("{-", "-}")]
            } else {
                &[]
            },
            ..d
        },
        "ini" | "cfg" | "properties" | "asm" | "s" | "lisp" | "clj" | "cljs" | "scm" | "rkt"
        | "desktop" => Spec {
            line_comments: &[";"],
            block_comments: &[],
            ..d
        },
        "erl" | "hrl" | "tex" | "sty" | "pro" => Spec {
            line_comments: &["%"],
            block_comments: &[],
            ..d
        },
        "bat" | "cmd" => Spec {
            line_comments: &[],
            block_comments: &[],
            line_start_comments: &["::", "rem ", "REM "],
            ..d
        },
        "html" | "htm" | "xml" | "svg" | "xhtml" => Spec {
            line_comments: &[],
            block_comments: &[("<!--", "-->")],
            ..d
        },
        "vue" | "svelte" | "astro" | "cshtml" | "razor" | "mdx" | "twig" | "liquid" => Spec {
            line_comments: &["//"],
            block_comments: &[("<!--", "-->")],
            ..d
        },
        "php" => Spec {
            line_comments: &["//", "#"],
            ..d
        },
        _ => d,
    }
}

// ---------------------------------------------------------------------------
// Brace-balanced scanning (the heart of the resolver)
// ---------------------------------------------------------------------------

/// Test/diagnostic helper: brace pairs for an extension (no path context).
#[cfg(test)]
pub(crate) fn brace_pairs_for_ext(entries: &[LineEntry], ext: &str) -> Vec<(usize, usize)> {
    let name = format!("x.{ext}");
    let path = Path::new(&name);
    scan_all(entries, &spec_for(ext, &path)).0
}

/// Lexically scan `entries` once, returning merged brace pairs `(start, end)`
/// (0-based, inclusive) — pairs joined by a same-line `} … {` bridge
/// (`} else {`, `} catch (E e) {`, `} while (…) {`) are merged into a single
/// span — plus resolved heredoc `(operator_line, terminator_line)` ranges.
fn scan_all(entries: &[LineEntry], spec: &Spec) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let mut scanner = BraceScanner::new(spec);
    for (li, entry) in entries.iter().enumerate() {
        scanner.scan_line(li, &entry.content);
    }
    scanner.finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Code,
    /// Inside a JS/Go template literal; `${ … }` expressions are scanned as
    /// code via a parallel mode stack.
    Template,
}

struct PendingHeredoc {
    delim: String,
    strip_tabs: bool,
    /// Line index containing the `<<EOF` operator.
    start: usize,
}

struct BraceScanner<'a> {
    spec: &'a Spec,
    /// Mode stack. `Code` entries pushed by `${` carry `Some(depth)` in
    /// `expr_depths`; the bottom `Code` carries `None`.
    modes: Vec<Mode>,
    expr_depths: Vec<Option<usize>>,
    /// Lines containing an unmatched `{`.
    stack: Vec<usize>,
    raw_pairs: Vec<(usize, usize)>,
    /// Lines where a `}` is followed by a `{` on the same line with only
    /// connector text between (`} else {`).
    bridge: HashSet<usize>,
    // Persistent multi-line states.
    block_comment_end: Option<&'static [u8]>,
    in_dq: bool,
    dq_esc: bool,
    in_triple: Option<&'static [u8]>,
    in_raw: Option<usize>,
    tmpl_esc: bool,
    // Per-line states (reset at each line start).
    in_sq: bool,
    sq_esc: bool,
    in_regex: bool,
    regex_class: bool,
    regex_esc: bool,
    last_sig: Option<u8>,
    word: Vec<u8>,
    word_active: bool,
    line_had_close: bool,
    line_close_pos: usize,
    heredocs: VecDeque<PendingHeredoc>,
    pending_heredocs: Vec<PendingHeredoc>,
    /// `(operator_line, terminator_line)` ranges for resolved heredocs —
    /// fallback spans for anchors inside heredoc bodies (Dockerfile
    /// `RUN <<EOF`, shell `cat <<EOF`) where no brace pair applies.
    heredoc_ranges: Vec<(usize, usize)>,
    here_string_end: Option<&'static str>,
}

impl<'a> BraceScanner<'a> {
    fn new(spec: &'a Spec) -> Self {
        Self {
            spec,
            modes: vec![Mode::Code],
            expr_depths: vec![None],
            stack: Vec::new(),
            raw_pairs: Vec::new(),
            bridge: HashSet::new(),
            block_comment_end: None,
            in_dq: false,
            dq_esc: false,
            in_triple: None,
            in_raw: None,
            tmpl_esc: false,
            in_sq: false,
            sq_esc: false,
            in_regex: false,
            regex_class: false,
            regex_esc: false,
            last_sig: None,
            word: Vec::new(),
            word_active: false,
            line_had_close: false,
            line_close_pos: 0,
            heredocs: VecDeque::new(),
            pending_heredocs: Vec::new(),
            heredoc_ranges: Vec::new(),
            here_string_end: None,
        }
    }

    fn scan_line(&mut self, li: usize, content: &str) {
        let bytes = content.as_bytes();

        // ---- whole-line skips ------------------------------------------------
        // Heredoc body / terminator: never code.
        if !self.heredocs.is_empty() {
            let t = content.trim();
            let tt = content.trim_start_matches('\t');
            if let Some(front) = self.heredocs.front() {
                if t == front.delim || (front.strip_tabs && tt == front.delim) {
                    let start = front.start;
                    self.heredocs.pop_front();
                    self.heredoc_ranges.push((start, li));
                }
            }
            return;
        }
        // PowerShell here-string body / terminator.
        if let Some(end) = self.here_string_end {
            if content.trim_start().starts_with(end) {
                self.here_string_end = None;
            }
            return;
        }
        if self.spec.ps_here_string {
            let t = content.trim_start();
            if t.starts_with("@\"") {
                self.here_string_end = Some("\"@");
                return;
            }
            if t.starts_with("@'") {
                self.here_string_end = Some("'@");
                return;
            }
        }
        // Zig multiline string content lines start with `\\`.
        if self.spec.zig_ml_string && content.trim_start().starts_with("\\\\") {
            return;
        }
        // Batch-style comments recognized only at line start.
        if !self.spec.line_start_comments.is_empty() {
            let t = content.trim_start();
            if self
                .spec
                .line_start_comments
                .iter()
                .any(|p| t.starts_with(p))
            {
                return;
            }
        }

        // ---- per-line resets ---------------------------------------------------
        self.in_sq = false;
        self.sq_esc = false;
        self.in_regex = false;
        self.regex_class = false;
        self.regex_esc = false;
        self.last_sig = None;
        self.word.clear();
        self.word_active = false;
        self.line_had_close = false;
        self.line_close_pos = 0;
        self.pending_heredocs.clear();
        if !self.spec.persistent_dq {
            self.in_dq = false;
            self.dq_esc = false;
        }

        let mut i = 0usize;
        while i < bytes.len() {
            let c = bytes[i];

            // ---- template literal mode -------------------------------------
            if *self.modes.last().unwrap() == Mode::Template {
                if self.tmpl_esc {
                    self.tmpl_esc = false;
                } else if c == b'\\' {
                    self.tmpl_esc = true;
                } else if c == b'`' {
                    self.modes.pop();
                    self.expr_depths.pop();
                } else if c == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                    self.modes.push(Mode::Code);
                    self.expr_depths.push(Some(0));
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            // ---- persistent multi-line states (Code mode) -------------------
            if let Some(end) = self.block_comment_end {
                if i + end.len() <= bytes.len() && &bytes[i..i + end.len()] == end {
                    self.block_comment_end = None;
                    i += end.len();
                    continue;
                }
                i += 1;
                continue;
            }
            if let Some(hashes) = self.in_raw {
                if c == b'"' {
                    let mut ok = i + 1 + hashes <= bytes.len();
                    if ok {
                        for k in 0..hashes {
                            if bytes[i + 1 + k] != b'#' {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        self.in_raw = None;
                        i += 1 + hashes;
                        continue;
                    }
                }
                i += 1;
                continue;
            }
            if let Some(delim) = self.in_triple {
                if i + delim.len() <= bytes.len() && &bytes[i..i + delim.len()] == delim {
                    self.in_triple = None;
                    i += delim.len();
                    continue;
                }
                i += 1;
                continue;
            }
            if self.in_dq {
                if self.dq_esc {
                    self.dq_esc = false;
                } else if c == b'\\' {
                    self.dq_esc = true;
                } else if c == b'"' {
                    self.in_dq = false;
                }
                i += 1;
                continue;
            }
            if self.in_sq {
                if self.sq_esc {
                    self.sq_esc = false;
                } else if c == b'\\' {
                    self.sq_esc = true;
                } else if c == b'\'' {
                    self.in_sq = false;
                }
                i += 1;
                continue;
            }
            if self.in_regex {
                if self.regex_esc {
                    self.regex_esc = false;
                } else if c == b'\\' {
                    self.regex_esc = true;
                } else if c == b'[' {
                    self.regex_class = true;
                } else if c == b']' {
                    self.regex_class = false;
                } else if c == b'/' && !self.regex_class {
                    self.in_regex = false;
                }
                i += 1;
                continue;
            }

            // ---- clean code character ---------------------------------------
            // Line comments.
            if self
                .spec
                .line_comments
                .iter()
                .any(|p| bytes[i..].starts_with(p.as_bytes()))
            {
                break;
            }
            // Block comments.
            let mut entered_comment = false;
            for (s, e) in self.spec.block_comments {
                if bytes[i..].starts_with(s.as_bytes()) {
                    self.block_comment_end = Some(e.as_bytes());
                    i += s.len();
                    entered_comment = true;
                    break;
                }
            }
            if entered_comment {
                continue;
            }
            // Heredoc operator `<<EOF` / `<<-'EOF'` / `<<~"EOF"`.
            if self.spec.heredoc && c == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'<' {
                if let Some((delim, strip_tabs, next)) = parse_heredoc_op(bytes, i) {
                    self.pending_heredocs.push(PendingHeredoc {
                        delim,
                        strip_tabs,
                        start: li,
                    });
                    i = next;
                    continue;
                }
            }
            // Rust raw strings: r"…", r#"…"#, br#"…"#.
            if self.spec.raw_string {
                if let Some((hashes, next)) = match_raw_string_prefix(bytes, i) {
                    self.in_raw = Some(hashes);
                    i = next;
                    continue;
                }
            }
            // Python/Ruby triple-quoted strings.
            if self.spec.triple_quote {
                if bytes[i..].starts_with(b"'''") {
                    self.in_triple = Some(b"'''");
                    i += 3;
                    continue;
                }
                if bytes[i..].starts_with(b"\"\"\"") {
                    self.in_triple = Some(b"\"\"\"");
                    i += 3;
                    continue;
                }
            }
            // Double-quoted strings.
            if c == b'"' && self.spec.dq {
                let opens = if self.spec.persistent_dq {
                    true
                } else {
                    has_closing_quote(bytes, i + 1, b'"')
                };
                if opens {
                    self.in_dq = true;
                }
                self.note_punct(c);
                i += 1;
                continue;
            }
            // Single-quoted strings / char literals / apostrophes.
            if c == b'\'' && self.spec.sq != SqMode::Off {
                let opens = match self.spec.sq {
                    SqMode::RustChar => rust_char_literal_at(bytes, i),
                    SqMode::Smart => has_closing_quote(bytes, i + 1, b'\''),
                    SqMode::Off => false,
                };
                if opens {
                    self.in_sq = true;
                }
                self.note_punct(c);
                i += 1;
                continue;
            }
            // Template literals / raw strings.
            if c == b'`' && self.spec.backtick {
                self.modes.push(Mode::Template);
                self.expr_depths.push(None);
                self.note_punct(c);
                i += 1;
                continue;
            }
            // Regex literals (JS/TS family). `//` and `/*` were handled above.
            // `</` is never a regex: in JSX/TSX it opens a closing tag
            // (`</div>`), and treating it as a regex would swallow the rest
            // of the line — including `}` braces — corrupting the pairing.
            if c == b'/'
                && self.spec.regex
                && !(i > 0 && bytes[i - 1] == b'<')
                && self.regex_allowed()
            {
                self.in_regex = true;
                self.regex_class = false;
                self.regex_esc = false;
                self.note_punct(c);
                i += 1;
                continue;
            }
            // Braces.
            if c == b'{' || c == b'}' {
                self.note_punct(c);
                let is_expr = matches!(self.expr_depths.last(), Some(Some(_)));
                if c == b'{' {
                    if is_expr {
                        if let Some(Some(d)) = self.expr_depths.last_mut() {
                            *d += 1;
                        }
                    } else {
                        self.stack.push(li);
                        // `} … {` bridge detection for else/catch merging.
                        if self.line_had_close {
                            let between = &bytes[self.line_close_pos + 1..i];
                            if bridge_between_ok(between) {
                                self.bridge.insert(li);
                            }
                        }
                    }
                } else {
                    if is_expr {
                        if let Some(Some(d)) = self.expr_depths.last_mut() {
                            if *d == 0 {
                                // `}` closing `${` — resume template mode.
                                self.modes.pop();
                                self.expr_depths.pop();
                            } else {
                                *d -= 1;
                            }
                        }
                    } else {
                        self.line_had_close = true;
                        self.line_close_pos = i;
                        if let Some(s) = self.stack.pop() {
                            self.raw_pairs.push((s, li));
                        }
                    }
                }
                i += 1;
                continue;
            }

            // Ordinary code character: identifiers feed the regex heuristic,
            // everything else is a significant punct.
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
                if !self.word_active {
                    self.word.clear();
                    self.word_active = true;
                }
                self.word.push(c);
            } else if !c.is_ascii_whitespace() {
                self.note_punct(c);
            }
            i += 1;
        }

        // Heredoc bodies start on the NEXT line.
        self.heredocs.extend(self.pending_heredocs.drain(..));
    }

    /// Track the last significant code token for the regex heuristic.
    fn note_punct(&mut self, c: u8) {
        self.last_sig = Some(c);
        self.word.clear();
        self.word_active = false;
    }

    /// Whether a `/` at the current position can start a regex literal.
    fn regex_allowed(&self) -> bool {
        if !self.word.is_empty() {
            return matches!(
                std::str::from_utf8(&self.word).unwrap_or(""),
                "return"
                    | "typeof"
                    | "instanceof"
                    | "case"
                    | "delete"
                    | "void"
                    | "in"
                    | "of"
                    | "new"
                    | "throw"
                    | "do"
                    | "else"
                    | "yield"
                    | "await"
            );
        }
        match self.last_sig {
            None => true, // start of line: statement position
            Some(c) => matches!(
                c,
                b'(' | b','
                    | b'='
                    | b':'
                    | b'['
                    | b'!'
                    | b'&'
                    | b'|'
                    | b'?'
                    | b'{'
                    | b';'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'%'
                    | b'<'
                    | b'>'
                    | b'^'
                    | b'~'
            ),
        }
    }

    fn finish(self) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
        (
            merge_bridge_pairs(&self.raw_pairs, &self.bridge),
            self.heredoc_ranges,
        )
    }
}

/// Text between a `}` and a following `{` on the same line qualifies as a
/// bridge only for connector material (`else`, `catch (E e)`, `while (…)`).
/// Commas/semicolons/brackets mean the braces are siblings (`}, {`), not a
/// compound statement, so no merge happens.
fn bridge_between_ok(b: &[u8]) -> bool {
    let start = b.iter().position(|&c| !c.is_ascii_whitespace());
    let Some(s) = start else { return false };
    let end = b.iter().rposition(|&c| !c.is_ascii_whitespace()).unwrap();
    let t = &b[s..=end];
    !t.is_empty()
        && t.iter().all(|&c| {
            !matches!(
                c,
                b',' | b';' | b'{' | b'}' | b'[' | b']' | b'"' | b'\'' | b'`'
            )
        })
}

/// Union-find merge of brace pairs joined by `} … {` bridge lines.
fn merge_bridge_pairs(pairs: &[(usize, usize)], bridge: &HashSet<usize>) -> Vec<(usize, usize)> {
    if pairs.is_empty() || bridge.is_empty() {
        return pairs.to_vec();
    }
    let n = pairs.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    // Index pairs by end line and start line.
    let mut end_map: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    let mut start_map: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for (idx, &(s, e)) in pairs.iter().enumerate() {
        end_map.entry(e).or_default().push(idx);
        start_map.entry(s).or_default().push(idx);
    }
    for &line in bridge {
        let (Some(enders), Some(starters)) = (end_map.get(&line), start_map.get(&line)) else {
            continue;
        };
        for &e_idx in enders {
            for &s_idx in starters {
                let (a, b) = (find(&mut parent, e_idx), find(&mut parent, s_idx));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    // Group by root → (min start, max end).
    let mut spans: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::new();
    for (idx, &(s, e)) in pairs.iter().enumerate() {
        let root = find(&mut parent, idx);
        spans
            .entry(root)
            .and_modify(|(ms, me)| {
                *ms = (*ms).min(s);
                *me = (*me).max(e);
            })
            .or_insert((s, e));
    }
    let mut out: Vec<(usize, usize)> = spans.into_values().collect();
    out.sort_unstable();
    out
}

// ---------------------------------------------------------------------------
// Small lexical helpers
// ---------------------------------------------------------------------------

/// Does an unescaped `quote` appear later on this line (bytes slice)?
fn has_closing_quote(bytes: &[u8], from: usize, quote: u8) -> bool {
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == b'\\' {
            j += 2;
            continue;
        }
        if bytes[j] == quote {
            return true;
        }
        j += 1;
    }
    false
}

/// Rust char-literal detection: `'x'`, `'\n'`, `b'x'` — but never `'a`
/// lifetimes or `'static`.
fn rust_char_literal_at(bytes: &[u8], i: usize) -> bool {
    debug_assert_eq!(bytes[i], b'\'');
    match bytes.get(i + 1) {
        Some(b'\\') => {
            // Escaped char literal: `'\n'`, `'\''`, `'\\'` — find the closing
            // quote within a short window, escape-aware.
            let mut j = i + 2;
            let limit = (i + 12).min(bytes.len());
            while j < limit {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'\'' {
                    return true;
                }
                j += 1;
            }
            false
        }
        Some(_) => bytes.get(i + 2) == Some(&b'\''),
        None => false,
    }
}

/// Match a Rust raw-string opener at `i`: `r"`, `r#"`, `br#"`, `cr##"` …
/// Returns `(hash_count, index_after_opening_quote)`.
fn match_raw_string_prefix(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    // The `r`/`br`/`cr` must not be the tail of a longer identifier.
    if i > 0 {
        let p = bytes[i - 1];
        if p.is_ascii_alphanumeric() || p == b'_' {
            return None;
        }
    }
    let mut j = i;
    if bytes[j] == b'b' || bytes[j] == b'c' {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] != b'r' {
        return None;
    }
    j += 1;
    let mut hashes = 0usize;
    while j < bytes.len() && bytes[j] == b'#' {
        hashes += 1;
        j += 1;
    }
    if j < bytes.len() && bytes[j] == b'"' {
        Some((hashes, j + 1))
    } else {
        None
    }
}

/// Parse a heredoc operator at `i` (pointing at the first `<` of `<<`).
/// Returns `(delimiter, strip_tabs, index_after_operator)`.
fn parse_heredoc_op(bytes: &[u8], i: usize) -> Option<(String, bool, usize)> {
    let mut j = i + 2;
    if j >= bytes.len() || bytes[j] == b'<' {
        return None; // `<<<` herestring or lone `<`
    }
    let mut strip_tabs = false;
    if j < bytes.len() && (bytes[j] == b'-' || bytes[j] == b'~') {
        strip_tabs = true;
        j += 1;
    }
    // Optional quoting of the delimiter.
    if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
        j += 1;
    }
    let start = j;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    if j == start {
        return None;
    }
    let delim = String::from_utf8_lossy(&bytes[start..j]).into_owned();
    // Optional closing quote.
    if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
        j += 1;
    }
    Some((delim, strip_tabs, j))
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// Strip a trailing `#`/`--` line comment (approximate — strings not tracked)
/// so header detection sees only code.
fn code_part<'a>(line: &'a str, spec: &Spec) -> &'a str {
    let t = line.trim_start();
    for p in spec.line_comments {
        if let Some(idx) = find_comment_start(t, p) {
            return &t[..idx];
        }
    }
    t
}

/// Find the first position where a line-comment prefix starts, requiring the
/// prefix to be at line start or preceded by whitespace (so `a#b` in Perl or
/// `x--y` in Lua don't count, while `code # comment` does).
fn find_comment_start(t: &str, prefix: &str) -> Option<usize> {
    let pb = prefix.as_bytes();
    let tb = t.as_bytes();
    let mut i = 0;
    while i + pb.len() <= tb.len() {
        if &tb[i..i + pb.len()] == pb {
            if i == 0 || tb[i - 1].is_ascii_whitespace() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Is this line entirely a comment (for indent-scan skipping)?
fn is_comment_line(trimmed: &str, spec: &Spec) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    for p in spec.line_comments {
        if trimmed.starts_with(p) {
            return true;
        }
    }
    for p in spec.line_start_comments {
        if trimmed.starts_with(p) {
            return true;
        }
    }
    false
}

/// Is this line a doc/comment/attribute/decorator line for the upward
/// extension (e.g. `///`, `//`, `/*`, `*`, `#[`, `@dec`, `#`).
fn is_doc_line(content: &str, spec: &Spec) -> bool {
    let t = content.trim_start();
    if t.is_empty() {
        return false;
    }
    for p in spec.line_comments {
        if t.starts_with(p) {
            return true;
        }
    }
    for p in spec.line_start_comments {
        if t.starts_with(p) {
            return true;
        }
    }
    for p in spec.doc_extra {
        if t.starts_with(p) {
            return true;
        }
    }
    false
}

/// Does `trimmed` end with a signature-continuation trigger? Used by the
/// forward scan in [`brace_resolve`] to skip signature/doc lines when the
/// anchor sits above a block. Deliberately narrow — `:` and `>` endings are
/// complete headers / HTML tags, not continuations.
fn sig_header_line(content: &str) -> bool {
    let t = content.trim_end();
    t.ends_with('(') || t.ends_with(',') || t.ends_with(')') || t.ends_with('=')
}

/// Unmatched closers on a line (approximate — strings/comments not stripped).
fn bracket_need(line: &str) -> i32 {
    let mut d = 0i32;
    for &b in line.as_bytes() {
        match b {
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            _ => {}
        }
    }
    -d.min(0) // number of unmatched closers, ≥ 0
}

/// Bracket delta of a line's code part for signature walking:
/// `unclosed_opens_at_eol - unmatched_closes_at_bol`. `fn f(` → +1,
/// `def f():` → 0 (parens balance on the line), a lone `)` → -1.
/// Approximate — strings are not tracked.
fn sig_bracket_delta(code: &str) -> i32 {
    let mut depth = 0i32;
    let mut unmatched_close = 0i32;
    for &b in code.as_bytes() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    unmatched_close += 1;
                }
            }
            _ => {}
        }
    }
    depth - unmatched_close
}

/// Walk a block start upward over signature-continuation lines.
///
/// Two mechanisms, both driven by evidence rather than line-ending
/// characters (the old `:`/`>`/`)` trigger list swallowed enclosing
/// headers — `def outer():` above `def inner():`, JSX `<main>` above
/// `{#if}`, CMake `if(…)` above `function(…)`):
///
/// 1. **Bracket walk** — when the resolved start is a signature TAIL with
///    unmatched closers (`) {`, `):`, ` b)`), consume lines above until the
///    brackets balance. If they never balance, the extension is reverted.
/// 2. **Allman bare `{`** — a line that is just `{` has its signature on the
///    lines above; step up once over a signature-looking line, then let the
///    bracket walk handle multi-line signatures (`)\n{`).
fn extend_signature_up(entries: &[LineEntry], mut start: usize, spec: &Spec) -> usize {
    let mut steps = 0;
    let origin = start;

    // Mechanism 2 first: bare `{` (Allman style).
    {
        let code = code_part(entries[start].content.trim(), spec);
        if code.trim_end() == "{" && start > 0 {
            let prev = entries[start - 1].content.trim();
            let pcode = code_part(prev, spec);
            let pl = pcode.trim_end();
            let has_alnum = pl.bytes().any(|b| b.is_ascii_alphanumeric());
            let trigger =
                pl.ends_with('(') || pl.ends_with(')') || pl.ends_with(',') || pl.ends_with('=');
            // Signature-like: alphanumeric line ending in a trigger, or a
            // line whose brackets are unbalanced (`)` tail of `f(\n…)`).
            if !pl.is_empty() && ((has_alnum && trigger) || sig_bracket_delta(pcode) != 0) {
                start -= 1;
                steps += 1;
            }
        }
    }

    // Mechanism 1: bracket-driven walk over signature tails.
    let mut need = {
        let code = code_part(entries[start].content.trim(), spec);
        let tail = code.trim_end();
        // The block's own opening `{` (last char) is not part of the
        // signature — exclude it before measuring unmatched closers.
        let body = if tail.ends_with('{') {
            &tail[..tail.len() - 1]
        } else {
            tail
        };
        -sig_bracket_delta(body)
    };
    while need > 0 && start > 0 && steps < 60 {
        start -= 1;
        steps += 1;
        let pcode = code_part(entries[start].content.trim(), spec);
        if pcode.trim().is_empty() {
            break; // signatures cannot span blank lines
        }
        need -= sig_bracket_delta(pcode);
    }
    if need > 0 {
        // Never balanced — the "signature" was unrelated lines above.
        // Revert rather than emit an over-wide span.
        start = origin;
    }
    start
}

/// For indent languages: extend a header start upward over an unclosed
/// bracket signature, e.g. Python `def f(\n    a,\n):` where the dedent scan
/// lands on the `):` line.
fn extend_indent_signature(entries: &[LineEntry], start: usize, spec: &Spec) -> usize {
    let need = bracket_need(&entries[start].content);
    if need == 0 {
        return start;
    }
    let mut depth = need;
    let mut s = start;
    for i in (0..start).rev().take(60) {
        let content = entries[i].content.trim();
        if content.is_empty() {
            break;
        }
        // Approximate bracket delta on the code part of the line.
        let code = code_part(content, spec);
        let mut opens = 0i32;
        let mut closes = 0i32;
        for &b in code.as_bytes() {
            match b {
                b'(' | b'[' | b'{' => opens += 1,
                b')' | b']' | b'}' => closes += 1,
                _ => {}
            }
        }
        depth += closes - opens;
        s = i;
        if depth <= 0 {
            break;
        }
    }
    if depth <= 0 { s } else { start }
}

/// Walk a block start upward over contiguous doc/comment/attribute lines.
fn extend_doc_up(entries: &[LineEntry], mut start: usize, spec: &Spec) -> usize {
    while start > 0 && is_doc_line(&entries[start - 1].content, spec) {
        start -= 1;
    }
    start
}

// ---------------------------------------------------------------------------
// Brace resolution
// ---------------------------------------------------------------------------

fn brace_resolve(
    entries: &[LineEntry],
    anchor_index: usize,
    spec: &Spec,
) -> Option<(usize, usize)> {
    let (pairs, heredocs) = scan_all(entries, spec);
    // Innermost pair containing the anchor.
    if let Some(p) = pairs
        .iter()
        .filter(|(s, e)| *s <= anchor_index && *e >= anchor_index)
        .max_by_key(|(s, _)| *s)
    {
        return Some(*p);
    }
    // Fallback: the anchor sits on doc/comment lines directly above a block,
    // or on a signature line whose `{` sits on a following line (Allman
    // style, multi-line signatures). Scan forward over doc/blank/
    // signature-continuation lines; the first real code line must open a
    // brace pair, which is then the resolved block.
    let anchor_t = entries[anchor_index].content.trim();
    let anchor_is_docish = anchor_t.is_empty() || is_doc_line(&entries[anchor_index].content, spec);
    let anchor_is_sig =
        sig_header_line(anchor_t) || anchor_t.ends_with(')') || anchor_t.ends_with(':');
    if anchor_is_docish || anchor_is_sig {
        let limit = entries.len().min(anchor_index + 61);
        for j in (anchor_index + 1)..limit {
            let t = entries[j].content.trim();
            if t.is_empty() || is_doc_line(&entries[j].content, spec) || sig_header_line(t) {
                continue;
            }
            if let Some(p) = pairs.iter().find(|(s, _)| *s == j) {
                return Some(*p);
            }
            break;
        }
    }
    // Last resort: the anchor sits inside a heredoc body (Dockerfile
    // `RUN <<EOF`, shell `cat <<EOF`) that no brace pair covers — resolve
    // to the whole heredoc command rather than failing or landing on an
    // arbitrary body line.
    if let Some(h) = heredocs
        .iter()
        .filter(|(s, e)| *s <= anchor_index && *e >= anchor_index)
        .max_by_key(|(s, _)| *s)
    {
        return Some(*h);
    }
    None
}

// ---------------------------------------------------------------------------
// Indentation resolution (Python, YAML, Verse, fallback)
// ---------------------------------------------------------------------------

/// Python continuation keywords: `else:`/`elif …:`/`except …:`/`finally:` at
/// the same indent continue a compound block instead of ending it.
fn is_indent_continuation(trimmed: &str) -> bool {
    for kw in ["else", "elif", "except", "finally"] {
        if let Some(rest) = trimmed.strip_prefix(kw) {
            if rest.is_empty() || rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
                return true;
            }
        }
    }
    false
}

/// Number of scannable lines: `entries` may carry a phantom trailing empty
/// entry produced by `split('\n')` on a file ending in a newline. It is not
/// a real line — a block must never "end" on it (that would swallow the
/// file's trailing newline when the span is deleted).
fn scan_len(entries: &[LineEntry]) -> usize {
    let n = entries.len();
    if n > 0 && entries[n - 1].content.is_empty() {
        n - 1
    } else {
        n
    }
}

/// Index of the last line of a (possibly multi-line) signature starting at
/// `start`: walks forward until the bracket depth returns to ≤ 0. For a
/// single-line header (`def f():`) this is `start` itself; for
/// `def foo(\n    a,\n):` it is the `):` line. Signature lines must be
/// excluded from the dedent scan — the `):` tail sits at header indent and
/// would otherwise be mistaken for the end of the block.
fn signature_tail(entries: &[LineEntry], start: usize, spec: &Spec) -> usize {
    let mut depth = 0i32;
    let mut tail = start;
    for i in start..entries.len() {
        let code = code_part(entries[i].content.trim(), spec);
        for &b in code.as_bytes() {
            match b {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                _ => {}
            }
        }
        tail = i;
        if depth <= 0 {
            break;
        }
    }
    tail
}

fn indent_resolve(
    entries: &[LineEntry],
    anchor_index: usize,
    spec: &Spec,
) -> Option<(usize, usize)> {
    let n = scan_len(entries);
    let anchor_t = entries[anchor_index].content.trim();
    // A blank or comment-only anchor is not part of any suite — resolve it
    // to itself (Bug #89-4: interior comment lines must not drag the whole
    // enclosing function into the span).
    if anchor_t.is_empty() || is_comment_line(anchor_t, spec) {
        return Some((anchor_index, anchor_index.min(n.saturating_sub(1))));
    }
    let anchor_indent = leading_ws(&entries[anchor_index].content);
    let anchor_code = code_part(anchor_t, spec);
    let anchor_is_header =
        spec.indent_colon_header && (anchor_indent == 0 || anchor_code.trim_end().ends_with(':'));

    if anchor_indent == 0 || anchor_is_header {
        // Header line: scan forward for the dedent boundary, starting after
        // any multi-line signature tail (`def foo(\n … \n):`).
        let tail = signature_tail(entries, anchor_index, spec);
        let mut end = n.saturating_sub(1);
        for i in (tail + 1)..n {
            let t = entries[i].content.trim();
            if t.is_empty() || is_comment_line(t, spec) {
                continue;
            }
            let ind = leading_ws(&entries[i].content);
            if ind <= anchor_indent {
                if spec.indent_continuations && ind == anchor_indent && is_indent_continuation(t) {
                    continue;
                }
                end = i.saturating_sub(1);
                break;
            }
        }
        return Some((anchor_index, end));
    }

    // Body line: scan backward for the nearest line with smaller indent —
    // the innermost enclosing suite header. Blank and comment lines are
    // skipped so a comment at column 0 can't hijack the header search.
    let mut start = None;
    for i in (0..anchor_index).rev() {
        let t = entries[i].content.trim();
        if t.is_empty() || is_comment_line(t, spec) {
            continue;
        }
        if leading_ws(&entries[i].content) < anchor_indent {
            start = Some(i);
            break;
        }
    }
    let mut start = start?;
    start = extend_indent_signature(entries, start, spec);
    let start_indent = leading_ws(&entries[start].content);
    // Skip the signature tail (`):` etc.) — it sits at header indent and
    // would falsely read as the dedent boundary.
    let tail = signature_tail(entries, start, spec);
    let mut end = n.saturating_sub(1);
    for i in (tail + 1)..n {
        let t = entries[i].content.trim();
        if t.is_empty() || is_comment_line(t, spec) {
            continue;
        }
        let ind = leading_ws(&entries[i].content);
        if ind <= start_indent {
            if spec.indent_continuations && ind == start_indent && is_indent_continuation(t) {
                continue;
            }
            end = i.saturating_sub(1);
            break;
        }
    }
    Some((start, end))
}

// ---------------------------------------------------------------------------
// Ruby `end`-keyword resolution
// ---------------------------------------------------------------------------

const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ",
];

fn ruby_opener_count(trimmed: &str) -> usize {
    for opener in RUBY_OPENERS {
        if trimmed.starts_with(opener) {
            return 1;
        }
    }
    0
}

fn ruby_resolve(entries: &[LineEntry], anchor_index: usize) -> Option<(usize, usize)> {
    let mut depth: isize = 0;
    let start = (0..=anchor_index).rev().find(|&i| {
        let trimmed = entries[i].content.trim();
        let end_count = if trimmed == "end" { 1 } else { 0 };
        let open_count = ruby_opener_count(trimmed);
        depth += end_count as isize;
        depth -= open_count as isize;
        open_count > 0 && depth <= 0
    })?;

    depth = 0;
    for i in start..entries.len() {
        let trimmed = entries[i].content.trim();
        let open_count = ruby_opener_count(trimmed);
        let end_count = if trimmed == "end" { 1 } else { 0 };
        depth += open_count as isize;
        depth -= end_count as isize;
        if i > start && depth <= 0 && trimmed == "end" {
            return Some((start, i));
        }
        if i == start && depth <= 0 {
            return Some((start, i));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Keyword-delimited resolution (CMake, PHP alternative syntax, Svelte)
// ---------------------------------------------------------------------------

/// Generic `opener … ender` keyword balancer (CMake, PHP alt syntax).
fn keyword_resolve(
    entries: &[LineEntry],
    anchor_index: usize,
    ext: &str,
) -> Option<(usize, usize)> {
    match ext {
        "svelte" => svelte_resolve(entries, anchor_index),
        "cmake" => keyword_balanced(entries, anchor_index, &cmake_opens, &cmake_closes),
        // PHP alternative syntax needs spec-aware open detection (`#`/`//`
        // comments must not confuse the `…):` header check).
        "php" => {
            let spec = spec_for("php", Path::new("x.php"));
            keyword_balanced(entries, anchor_index, &|t| php_opens(t, &spec), &php_closes)
        }
        _ => None,
    }
}

fn cmake_opens(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    for kw in [
        "function", "macro", "if", "foreach", "while", "block", "macro",
    ] {
        if let Some(rest) = l.strip_prefix(kw) {
            let r = rest.trim_start();
            if r.starts_with('(') {
                return true;
            }
        }
    }
    false
}

fn cmake_closes(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    for kw in [
        "endfunction",
        "endmacro",
        "endif",
        "endforeach",
        "endwhile",
        "endblock",
    ] {
        if l.starts_with(kw) {
            return true;
        }
    }
    false
}

fn php_opens(line: &str, spec: &Spec) -> bool {
    // Alternative syntax: `if ($x):`, `foreach (...):`, `while (...):` …
    let code = code_part(line.trim(), spec);
    let t = code.trim();
    if !t.ends_with(':') {
        return false;
    }
    let word_end = t
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(t.len());
    matches!(
        t[..word_end].to_ascii_lowercase().as_str(),
        "if" | "for" | "foreach" | "while" | "switch" | "declare"
    )
}

fn php_closes(t: &str) -> bool {
    let l = t.trim_start().to_ascii_lowercase();
    for kw in [
        "endif",
        "endfor",
        "endforeach",
        "endwhile",
        "endswitch",
        "enddeclare",
    ] {
        if l.starts_with(kw) {
            return true;
        }
    }
    false
}

/// Balance `opener`/`closer` line predicates around `anchor_index`.
fn keyword_balanced(
    entries: &[LineEntry],
    anchor_index: usize,
    opens: &dyn Fn(&str) -> bool,
    closes: &dyn Fn(&str) -> bool,
) -> Option<(usize, usize)> {
    // Backward scan for the enclosing opener (mirrors the Ruby algorithm).
    let mut depth: isize = 0;
    let start = (0..=anchor_index).rev().find(|&i| {
        let t = entries[i].content.trim();
        let ec = usize::from(closes(t)) as isize;
        let oc = usize::from(opens(t)) as isize;
        depth += ec;
        depth -= oc;
        oc > 0 && depth <= 0
    })?;

    depth = 0;
    for i in start..entries.len() {
        let t = entries[i].content.trim();
        let oc = usize::from(opens(t)) as isize;
        let ec = usize::from(closes(t)) as isize;
        depth += oc;
        depth -= ec;
        if i > start && depth <= 0 && closes(t) {
            return Some((start, i));
        }
        if i == start && depth <= 0 {
            return Some((start, i));
        }
    }
    None
}

/// Svelte `{#if}…{/if}`, `{#each}…{/each}`, `{#await}…{/await}`, `{#key}`.
fn svelte_resolve(entries: &[LineEntry], anchor_index: usize) -> Option<(usize, usize)> {
    fn opens(t: &str) -> bool {
        t.starts_with("{#") && !t.starts_with("{:/")
    }
    fn closes(t: &str) -> bool {
        t.starts_with("{/") && t.contains('}')
    }
    keyword_balanced(entries, anchor_index, &opens, &closes)
}

// ---------------------------------------------------------------------------
// Public helpers for find-block / editor
// ---------------------------------------------------------------------------

/// Use a [`BlockResolver`] to find the syntactic block around an anchor.
///
/// Returns `(language_name, start_index, end_index)` where indices are
/// 0-based (same as `LineEntry` indexing).
pub fn resolve_block_boundaries(
    entries: &[LineEntry],
    anchor_index: usize,
    path: &std::path::Path,
    resolver: &dyn BlockResolver,
) -> Result<(Option<String>, usize, usize), HashlineError> {
    let text: String = entries
        .iter()
        .map(|e| e.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let request = BlockResolverRequest {
        path: path.to_string_lossy().to_string(),
        text,
        line: anchor_index + 1,
    };

    let span = resolver
        .resolve(&request)
        .ok_or_else(|| HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        })?;

    // `BlockSpan` is 1-indexed; this function returns 0-based indices for
    // direct `entries` slicing.
    if span.start == 0 || span.end < span.start {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }
    let start = span.start - 1;
    let end = span.end - 1;

    if start >= entries.len() || end >= entries.len() || start > end {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = language_for_extension(extension);

    Ok((language, start, end))
}

pub(crate) fn language_for_extension(ext: &str) -> Option<String> {
    match ext {
        "rs" => Some("Rust".into()),
        "py" => Some("Python".into()),
        "js" => Some("JavaScript".into()),
        "ts" => Some("TypeScript".into()),
        "tsx" => Some("TSX".into()),
        "jsx" => Some("JSX".into()),
        "go" => Some("Go".into()),
        "rb" => Some("Ruby".into()),
        "verse" => Some("Verse".into()),
        "java" => Some("Java".into()),
        "c" => Some("C".into()),
        "cpp" | "hpp" => Some("C++".into()),
        "h" => Some("C/C++ Header".into()),
        "cs" => Some("C#".into()),
        "kt" | "kts" => Some("Kotlin".into()),
        "swift" => Some("Swift".into()),
        "scala" => Some("Scala".into()),
        "dart" => Some("Dart".into()),
        "zig" => Some("Zig".into()),
        "m" => Some("Objective-C".into()),
        "mm" => Some("Objective-C++".into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests — audit repros (bd-scanner-correctness-x5g / bd-block-engine-unify-qt9)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn entries_for(content: &str) -> Vec<LineEntry> {
        content
            .split('\n')
            .map(|s| LineEntry {
                content: s.to_string(),
                short_hash: 0,
            })
            .collect()
    }

    /// Resolve the 1-based anchor line for `path`; returns the 1-based
    /// inclusive `(start, end)` span.
    fn span_of(content: &str, anchor_line: usize, path: &str) -> Option<(usize, usize)> {
        let entries = entries_for(content);
        resolve_span(&entries, anchor_line - 1, Path::new(path)).map(|(s, e)| (s + 1, e + 1))
    }

    // -- Rust: lifetimes and char literals must not corrupt brace depth ----

    #[test]
    fn rust_lifetime_and_char_literals() {
        let src = "struct Foo<'a> {\n    s: &'a str,\n    c: char,\n}\n\nfn f() {\n    let x = 'a';\n    let y = '\'';\n    let z = b'x';\n}\n";
        // Anchor inside the struct: lifetime `'a` / `&'a str` must not open
        // a phantom string state.
        assert_eq!(span_of(src, 1, "x.rs"), Some((1, 4)));
        // Anchor inside fn: `'a'`, `'\''`, `b'x'` char literals are fine.
        assert_eq!(span_of(src, 6, "x.rs"), Some((6, 10)));
    }

    #[test]
    fn rust_escaped_char_and_byte() {
        let src = "fn f() {\n    match c {\n        '}' => 1,\n        '\\n' => 2,\n        _ => 3,\n    }\n}\n";
        // Innermost pair is the match block; char literals with braces
        // (`'}'`) must not corrupt the pairing.
        assert_eq!(span_of(src, 2, "x.rs"), Some((2, 6)));
    }

    // -- JS/TS: template literals, regex literals, JSX apostrophes ---------

    #[test]
    fn js_template_literal_braces_ignored() {
        let src = "function f() {\n  const s = `a } b { c`;\n  const t = `x ${ {a:1} } y`;\n  return 1;\n}\nfunction g() {\n  return 2;\n}\n";
        assert_eq!(span_of(src, 4, "x.js"), Some((1, 5)));
        assert_eq!(span_of(src, 7, "x.js"), Some((6, 8)));
    }

    #[test]
    fn js_regex_literal_braces_ignored() {
        let src = "function f() {\n  const re = /{/;\n  const re2 = /}/g;\n  const div = a / b;\n  return 1;\n}\nfunction g() {\n  return 9;\n}\n";
        assert_eq!(span_of(src, 5, "x.js"), Some((1, 6)));
        assert_eq!(span_of(src, 8, "x.js"), Some((7, 9)));
    }

    #[test]
    fn tsx_jsx_apostrophes_and_close_tags() {
        // `Don't` in JSX text and `</p>` closing tags must not be treated
        // as string opens or regex starts.
        let src = "export function Dashboard() {\n  const label = \"Don't panic\";\n  return (\n    <section>\n      {open && <p>Don't close</p>}\n    </section>\n  );\n}\n";
        assert_eq!(span_of(src, 2, "x.tsx"), Some((1, 8)));
        // The JSX expression `{open && …}` is its own innermost pair.
        assert_eq!(span_of(src, 5, "x.tsx"), Some((5, 5)));
        assert_eq!(span_of(src, 6, "x.tsx"), Some((1, 8)));
    }

    // -- Line comments per extension ---------------------------------------

    #[test]
    fn terraform_hash_comments_and_heredoc() {
        let src = "terraform {\n  required_version = \">= 1.5\"\n}\n\n# commented brace: resource \"bad\" {\nvariable \"env\" {\n  type = string\n  # } note\n}\n\nresource \"aws\" \"web\" {\n  user_data = <<-EOT\n    echo \"ready }\"\n  EOT\n}\n";
        assert_eq!(span_of(src, 1, "x.tf"), Some((1, 3)));
        // The `#` comment directly above the block is treated as its doc
        // line and included in the span.
        assert_eq!(span_of(src, 7, "x.tf"), Some((5, 9)));
        // Anchor inside the heredoc body resolves the enclosing resource.
        assert_eq!(span_of(src, 14, "x.tf"), Some((11, 15)));
    }

    #[test]
    fn graphql_hash_comment_inside_block() {
        let src = "type User {\n  userId: ID!\n  # } stray\n  name: String\n}\n";
        assert_eq!(span_of(src, 4, "x.graphql"), Some((1, 5)));
    }

    #[test]
    fn powershell_comments_and_heredoc() {
        let src = "function Deploy {\n    if ($Env -eq 'prod') {\n        Write-Output 'production'\n    } else {\n        Write-Output 'dev'\n    }\n    # } note\n}\n";
        // Innermost pair is the if/else (bridge-merged) block.
        assert_eq!(span_of(src, 3, "x.ps1"), Some((2, 6)));
        assert_eq!(span_of(src, 6, "x.ps1"), Some((2, 6)));
    }

    // -- Multiline string constructs ----------------------------------------

    #[test]
    fn shell_heredoc_braces_ignored() {
        let src = "deploy() {\n  cat > config.json <<'JSON'\n{\n  \"name\": \"app\"\n}\n# } note\nJSON\n  if [ \"$env\" = \"prod\" ]; then\n    echo prod\n  fi\n}\n";
        assert_eq!(span_of(src, 9, "x.sh"), Some((1, 11)));
    }

    #[test]
    fn dockerfile_run_heredoc_fallback() {
        let src = "FROM alpine\nRUN <<'EOF'\n#!/bin/sh\necho \"hi }\"\nEOF\nCMD [\"sh\"]\n";
        // Anchor inside the heredoc body: no brace pair covers it, so the
        // whole `RUN <<EOF … EOF` command is the resolved span.
        assert_eq!(span_of(src, 3, "Dockerfile"), Some((2, 5)));
        assert_eq!(span_of(src, 2, "Dockerfile"), Some((2, 5)));
    }

    #[test]
    fn zig_multiline_strings() {
        let src = "pub fn deploy() !void {\n    const msg =\n        \\\\ first {\n        \\\\ second }\n    while (true) {\n        break;\n    }\n}\n";
        // The `\\` string lines are not code — an anchor on one resolves the
        // enclosing function, and the while loop is its own innermost block.
        assert_eq!(span_of(src, 3, "x.zig"), Some((1, 8)));
        assert_eq!(span_of(src, 5, "x.zig"), Some((5, 7)));
    }

    #[test]
    fn kotlin_triple_quoted_string() {
        let src = "fun deploy() {\n    val banner = \"\"\"\n        {\n          \"state\": \"ready\"\n        }\n    \"\"\".trimIndent()\n    if (true) {\n        println()\n    }\n}\n";
        // A `{` inside the triple-quoted string is not a brace pair — the
        // anchor resolves the enclosing function.
        assert_eq!(span_of(src, 3, "x.kt"), Some((1, 10)));
        assert_eq!(span_of(src, 7, "x.kt"), Some((7, 9)));
    }

    #[test]
    fn rust_raw_string() {
        let src = "fn f() {\n    let s = r#\"{ not a brace }\"#;\n    let t = 1;\n}\n";
        assert_eq!(span_of(src, 3, "x.rs"), Some((1, 4)));
    }

    // -- Structural: same-line close/open bridges ---------------------------

    #[test]
    fn else_bridge_merges_into_one_block() {
        let src = "function f() {\n  if (x) {\n    a();\n  } else {\n    b();\n  }\n  c();\n}\n";
        // Anchor in the else arm: the whole if/else is one block.
        assert_eq!(span_of(src, 5, "x.js"), Some((2, 6)));
        assert_eq!(span_of(src, 3, "x.js"), Some((2, 6)));
    }

    #[test]
    fn catch_finally_bridge() {
        let src = "try {\n  a();\n} catch (e) {\n  b();\n} finally {\n  c();\n}\n";
        assert_eq!(span_of(src, 4, "x.js"), Some((1, 7)));
    }

    // -- Python: body anchors scan back to the enclosing header -------------

    #[test]
    fn python_body_anchor_finds_enclosing_def() {
        let src = "def outer():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        // Anchor on a body statement resolves the innermost suite.
        assert_eq!(span_of(src, 4, "x.py"), Some((3, 4)));
        assert_eq!(span_of(src, 5, "x.py"), Some((1, 5)));
    }

    #[test]
    fn python_comment_anchor_is_single_line() {
        // Bug #89-4: a comment-only anchor must not drag in the whole def.
        let src = "def hello():\n    # comment\n    x = 1\n    return x";
        assert_eq!(span_of(src, 2, "x.py"), Some((2, 2)));
    }

    #[test]
    fn python_nested_def_does_not_swallow_outer() {
        let src = "def outer():\n    def inner():\n        x = 1\n        return x\n    y = inner()\n    return y\n";
        assert_eq!(span_of(src, 3, "x.py"), Some((2, 4)));
    }

    #[test]
    fn python_multiline_signature() {
        let src = "def foo(\n    a,\n    b,\n):\n    return a + b\n";
        assert_eq!(span_of(src, 5, "x.py"), Some((1, 5)));
    }

    // -- Keyword-delimited fallbacks -----------------------------------------

    #[test]
    fn svelte_if_block() {
        let src = "<script>\n  let x = 0;\n</script>\n\n<main>\n  {#if x > 0}\n    <p>hi</p>\n  {/if}\n</main>\n";
        assert_eq!(span_of(src, 7, "x.svelte"), Some((6, 8)));
    }

    #[test]
    fn cmake_function_endfunction() {
        let src = "cmake_minimum_required(VERSION 3.28)\n\nfunction(deploy_app)\n    if(X)\n        message(STATUS \"p\")\n    else()\n        message(STATUS \"d\")\n    endif()\nendfunction()\n";
        // Anchor inside the if: resolves the if/else/endif block.
        assert_eq!(span_of(src, 5, "x.cmake"), Some((4, 8)));
        // Anchor on endfunction resolves the whole function.
        assert_eq!(span_of(src, 9, "x.cmake"), Some((3, 9)));
    }

    #[test]
    fn php_alternative_syntax() {
        let src = "<?php\nif ($x):\n    echo 1;\nelse:\n    echo 2;\nendif;\n";
        assert_eq!(span_of(src, 3, "x.php"), Some((2, 6)));
    }

    #[test]
    fn ruby_end_block() {
        let src = "def hello\n  x = 1\n  if true\n    puts 'ok'\n  end\nend\n";
        assert_eq!(span_of(src, 4, "x.rb"), Some((3, 5)));
    }

    // -- Fail-loud cases ------------------------------------------------------

    #[test]
    fn unbalanced_braces_fail_closed() {
        let src = "fn f() {\n    let x = 1;\n";
        assert_eq!(span_of(src, 2, "x.rs"), None);
    }

    #[test]
    fn phantom_trailing_line_never_in_span() {
        // File ends with a newline: the split('\n') phantom "" entry must
        // not be swallowed into the block span.
        let src = "def f():\n    pass\n";
        let entries = entries_for(src);
        assert_eq!(entries.len(), 3); // includes phantom ""
        assert_eq!(resolve_span(&entries, 0, Path::new("x.py")), Some((0, 1)));
    }
}
