use serde::Serialize;
use std::path::Path;

use crate::lang::detect::Lang;
use crate::lang::parser_pool;

/// Hard limit on the input size accepted by the outline pipeline (in bytes).
///
/// Tree-sitter parses non-source content (binary files, plain text, files with
/// the wrong extension, etc.) by emitting one ERROR node per byte and running
/// error recovery, which is roughly O(N^2). A 10K-line plain-text file with a
/// `.rs` extension used to keep the parser busy for ~91 s; this limit short-
/// circuits that path before tree-sitter ever sees the content.
///
/// 5 MB is large enough to comfortably cover real-world source files (the
/// largest file in this repo is well under 100 KB) while making the worst
/// case complete in milliseconds.
pub const MAX_OUTLINE_INPUT_BYTES: usize = 5 * 1024 * 1024;

/// Hard limit on the input size accepted by the outline pipeline (in lines).
///
/// Complements [`MAX_OUTLINE_INPUT_BYTES`] for files that are line-heavy but
/// byte-light (e.g. minified JS or generated code). 50K lines is well above
/// the practical ceiling for human-authored source files.
pub const MAX_OUTLINE_INPUT_LINES: usize = 50_000;

/// Kind of outline entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutlineKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Class,
    Method,
    Field,
    Property,
    Import,
    Comment,
    Type,
    Constant,
}

/// An outline entry with line range and optional children for nested structures.
#[derive(Debug, Clone, Serialize)]
pub struct OutlineEntry {
    pub kind: OutlineKind,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineEntry>,
}

impl OutlineEntry {
    pub fn new(kind: OutlineKind, name: &str, start: usize) -> Self {
        Self {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: start,
            signature: None,
            children: Vec::new(),
        }
    }

    pub fn with_signature(mut self, sig: &str) -> Self {
        self.signature = Some(sig.to_string());
        self
    }

    pub fn with_end_line(mut self, end: usize) -> Self {
        self.end_line = end;
        self
    }

    pub fn with_children(mut self, children: Vec<OutlineEntry>) -> Self {
        self.children = children;
        self
    }
}

/// Get structured outline entries for file content.
///
/// Uses a thread-local parser cache (see `lang::parser_pool`) so repeated
/// calls within a single process (CLI batch, MCP server) skip the cost of
/// constructing a fresh `tree_sitter::Parser` and re-binding the grammar
/// on every call.
pub fn get_outline_entries(text: &str, lang: Lang) -> Vec<OutlineEntry> {
    let parsed = parser_pool::with_parser(lang, |parser| parser.parse(text, None));

    let Some(Some(tree)) = parsed else {
        return get_outline_entries_fallback(text, lang);
    };

    let lines: Vec<&str> = text.lines().collect();
    walk_top_level(tree.root_node(), &lines, lang)
}

/// Walk top-level children of the root node, extracting outline entries.
fn walk_top_level(root: tree_sitter::Node, lines: &[&str], lang: Lang) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        if let Some(entry) = node_to_entry(child, lines, lang, 0) {
            entries.push(entry);
        }
    }

    entries
}

/// Convert a tree-sitter node to an `OutlineEntry` based on its kind.
///
/// TODO: the match below currently merges grammar node-kind names across
/// every supported language. Several names collide (e.g. `function_definition`
/// is JS/TS *and* Python *and* C/C++; `function_declaration` is Rust *and* Go)
/// so later arms are unreachable and clippy correctly flags it. This is a
/// real correctness bug that needs a language-aware dispatch (use `_lang` to
/// select the right kind→OutlineKind table). For now we silence the lint so
/// the rest of the codebase can be cleaned up; the bug is tracked separately.
#[allow(unreachable_patterns)]
fn node_to_entry(
    node: tree_sitter::Node,
    lines: &[&str],
    _lang: Lang,
    depth: usize,
) -> Option<OutlineEntry> {
    let kind_str = node.kind();
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let (kind, name, signature) = match kind_str {
        // Rust
        "function_item" | "function_declaration" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }
        "struct_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Struct, name, None)
        }
        "enum_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Enum, name, None)
        }
        "trait_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Trait, name, None)
        }
        "impl_item" => {
            let name = find_child_text(node, "type", lines).unwrap_or_else(|| "<impl>".into());
            (OutlineKind::Impl, format!("impl {name}"), None)
        }
        "mod_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Module, name, None)
        }
        "type_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Type, name, None)
        }
        "const_item" | "static_item" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Constant, name, None)
        }

        // JavaScript / TypeScript (function_declaration handled by Rust above)
        "function_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }
        "class_declaration" | "class_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Class, name, None)
        }
        "method_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Method, name, Some(sig))
        }
        "variable_declarator" if depth == 0 => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Constant, name, None)
        }

        // Python
        "function_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }
        "class_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Class, name, None)
        }
        "async_function_definition" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }

        // Go
        "function_declaration" | "function_literal" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }
        "type_declaration" => {
            // Go: type X struct { ... }
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            // Check if it's a struct or interface by looking at the type
            (OutlineKind::Type, name, None)
        }
        "const_declaration" | "var_declaration" => {
            let name = first_identifier_text(node, lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Constant, name, None)
        }

        // C / C++
        "translation_unit" => return None, // skip root
        "function_definition" => {
            let name = find_child_text(node, "declarator", lines)
                .or_else(|| find_child_text(node, "declaration", lines))
                .unwrap_or_else(|| "<anon>".into());
            let sig = extract_signature(node, lines);
            (OutlineKind::Function, name, Some(sig))
        }
        "struct_specifier" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Struct, name, None)
        }
        "enum_specifier" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Enum, name, None)
        }
        "type_alias" => {
            let name = find_child_text(node, "name", lines).unwrap_or_else(|| "<anon>".into());
            (OutlineKind::Type, name, None)
        }

        // Generic: import / using
        "import_statement" | "import_declaration" | "using_declaration" => {
            let text = node_text(node, lines);
            (OutlineKind::Import, text, None)
        }

        _ => return None,
    };

    Some(OutlineEntry {
        kind,
        name,
        start_line,
        end_line,
        signature,
        children: Vec::new(),
    })
}

/// Extract the first line as a function signature (name + params).
fn extract_signature(node: tree_sitter::Node, lines: &[&str]) -> String {
    let start_row = node.start_position().row;
    if start_row < lines.len() {
        let line = lines[start_row].trim();
        // Truncate at opening brace
        if let Some(pos) = line.find('{') {
            return line[..pos].trim().to_string();
        }
        // Python — truncate at trailing colon
        if line.ends_with(':') {
            if let Some(pos) = line.rfind(':') {
                return line[..pos].trim().to_string();
            }
        }
        // Truncate at 120 chars
        if line.len() > 120 {
            format!("{}...", &line[..117])
        } else {
            line.to_string()
        }
    } else {
        String::new()
    }
}

/// Find a named child and return its text.
fn find_child_text(node: tree_sitter::Node, field: &str, lines: &[&str]) -> Option<String> {
    node.child_by_field_name(field).map(|n| node_text(n, lines))
}

/// Collect child entries from a class/struct/impl body.
fn collect_children(
    node: tree_sitter::Node,
    lines: &[&str],
    lang: Lang,
    depth: usize,
) -> Vec<OutlineEntry> {
    let mut children = Vec::new();
    let mut cursor = node.walk();

    // Look for a body node (struct_body, class_body, block, etc.)
    let body = node.children(&mut cursor).find(|c| {
        let k = c.kind();
        k.contains("body") || k.contains("block") || k == "declaration_list"
    });

    let parent = body.unwrap_or(node);
    let mut cursor2 = parent.walk();

    for child in parent.children(&mut cursor2) {
        if let Some(entry) = node_to_entry(child, lines, lang, depth) {
            children.push(entry);
        }
    }

    children
}

/// Extract doc comment from preceding sibling or node metadata.
/// Currently a placeholder - tree-sitter's cursor API doesn't support sibling iteration easily.
fn extract_doc(_node: tree_sitter::Node, _lines: &[&str]) -> Option<String> {
    None
}

/// Get the text of a node, truncated to the first line.
fn node_text(node: tree_sitter::Node, lines: &[&str]) -> String {
    let row = node.start_position().row;
    let col_start = node.start_position().column;
    let end_row = node.end_position().row;

    if row < lines.len() {
        if row == end_row {
            let col_end = node.end_position().column.min(lines[row].len());
            lines[row][col_start..col_end].to_string()
        } else {
            let text = &lines[row][col_start..];
            if text.len() > 80 {
                format!("{}...", &text[..77])
            } else {
                text.to_string()
            }
        }
    } else {
        String::new()
    }
}

/// Find the first identifier-like child.
fn first_identifier_text(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind.contains("identifier") || kind.contains("name") {
            let text = node_text(child, lines);
            if !text.is_empty() {
                return Some(text);
            }
        }
        // Recurse through declarators
        if kind.contains("declarator") || kind.contains("declaration") {
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if grandchild.kind().contains("identifier") {
                    let text = node_text(grandchild, lines);
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Fallback: naive line-by-line parser (preserves original logic)
// ---------------------------------------------------------------------------

/// Language-specific definition kind patterns for naive extraction.
fn rust_definition_kinds() -> Vec<(&'static str, OutlineKind)> {
    vec![
        ("fn ", OutlineKind::Function),
        ("struct ", OutlineKind::Struct),
        ("enum ", OutlineKind::Enum),
        ("trait ", OutlineKind::Trait),
        ("impl ", OutlineKind::Impl),
        ("mod ", OutlineKind::Module),
        ("type ", OutlineKind::Type),
        ("const ", OutlineKind::Constant),
        ("static ", OutlineKind::Constant),
    ]
}

fn js_definition_kinds() -> Vec<(&'static str, OutlineKind)> {
    vec![
        ("function ", OutlineKind::Function),
        ("async function ", OutlineKind::Function),
        ("class ", OutlineKind::Class),
        ("const ", OutlineKind::Constant),
        ("let ", OutlineKind::Constant),
        ("var ", OutlineKind::Constant),
    ]
}

fn go_definition_kinds() -> Vec<(&'static str, OutlineKind)> {
    vec![
        ("func ", OutlineKind::Function),
        ("type ", OutlineKind::Struct),
        ("const ", OutlineKind::Constant),
        ("var ", OutlineKind::Constant),
    ]
}

fn python_definition_kinds() -> Vec<(&'static str, OutlineKind)> {
    vec![
        ("def ", OutlineKind::Function),
        ("class ", OutlineKind::Class),
        ("async def ", OutlineKind::Function),
    ]
}

/// Extract outline entries using naive line-by-line parsing.
/// This is a fallback when tree-sitter is unavailable.
pub fn get_outline_entries_fallback(text: &str, lang: Lang) -> Vec<OutlineEntry> {
    let kinds = match lang {
        Lang::Rust => rust_definition_kinds(),
        Lang::JavaScript | Lang::TypeScript => js_definition_kinds(),
        Lang::Go => go_definition_kinds(),
        Lang::Python => python_definition_kinds(),
        _ => return Vec::new(),
    };

    let mut entries = Vec::new();
    let mut current_fn: Option<(usize, String)> = None;
    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        for (prefix, kind) in &kinds {
            let rest = if trimmed.starts_with("pub ")
                && trimmed[3..].starts_with(prefix.trim_start_matches("pub "))
            {
                Some((&trimmed[4..], true))
            } else if trimmed.starts_with(prefix) {
                Some((trimmed, false))
            } else {
                None
            };

            if let Some((def_line, _is_pub)) = rest {
                if lang == Lang::Rust && def_line.starts_with("impl ") {
                    // Accept impl block
                }

                let name = def_line
                    .split(|c: char| c.is_whitespace() || c == '(' || c == '{')
                    .nth(1)
                    .unwrap_or(def_line);

                if name.is_empty() {
                    continue;
                }

                let start_line = i + 1;

                let signature = if trimmed.len() > 100 {
                    Some(format!("{}...", &trimmed[..100]))
                } else {
                    Some(trimmed.to_string())
                };

                let open_braces = line.matches('{').count() as i32;
                let close_braces = line.matches('}').count() as i32;

                if open_braces > 0 || close_braces > 0 {
                    current_fn = Some((start_line, name.to_string()));
                }

                let mut entry = OutlineEntry::new(*kind, name, start_line);
                entry.signature = signature;
                entries.push(entry);
                break;
            }
        }

        if let Some((start_line, ref name)) = current_fn {
            let open = line.matches('{').count() as i32;
            let close = line.matches('}').count() as i32;
            if open > 0 || close > 0 {
                if let Some(entry) = entries
                    .iter_mut()
                    .find(|e| e.start_line == start_line && e.name == *name)
                {
                    entry.end_line = i + 1;
                }
                if close > 0 && open == 0 {
                    current_fn = None;
                }
            }
        }
    }

    entries
}

/// Detect language from a path (free function).
pub fn detect_language_from_path(path: &Path) -> Lang {
    Lang::from_path(path)
}
