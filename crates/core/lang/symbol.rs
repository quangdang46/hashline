use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::lang::detect::{Lang, detect_language_from_path};

/// Kind of symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SymbolKind {
    Function,
    Variable,
    Type,
    Struct,
    Enum,
    Trait,
    Module,
    Constant,
    Property,
    Method,
    Field,
}

/// A symbol occurrence (definition or reference).
#[derive(Debug, Clone, Serialize)]
pub struct SymbolOccurrence {
    pub kind: SymbolKind,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub is_def: bool,
}

/// Symbol search result.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolResult {
    pub query: String,
    pub matches: Vec<SymbolOccurrence>,
    pub total: usize,
}

impl SymbolResult {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_string(),
            matches: Vec::new(),
            total: 0,
        }
    }
}

/// Extract symbol definitions from source text.
pub fn extract_symbols(text: &str, lang: Lang, path: &Path) -> Vec<SymbolOccurrence> {
    let kinds = match lang {
        Lang::Rust => rust_symbol_kinds(),
        Lang::JavaScript | Lang::TypeScript => js_symbol_kinds(),
        Lang::Go => go_symbol_kinds(),
        Lang::Python => python_symbol_kinds(),
        _ => return Vec::new(),
    };

    let mut symbols = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        for (prefix, kind) in &kinds {
            // Check for definition patterns
            let rest = if trimmed.starts_with("pub ") && trimmed[4..].starts_with(prefix) {
                Some(&trimmed[4..])
            } else if trimmed.starts_with(prefix) {
                Some(trimmed)
            } else {
                None
            };

            if let Some(def) = rest {
                let name = def
                    .split(|c: char| c.is_whitespace() || c == '(' || c == '{' || c == '<')
                    .nth(1)
                    .unwrap_or(def);
                if !name.is_empty() {
                    symbols.push(SymbolOccurrence {
                        kind: kind.clone(),
                        name: name.to_string(),
                        file: path.to_string_lossy().into_owned(),
                        line: line_idx + 1,
                        col: trimmed.find(name).map(|p| p + 1).unwrap_or(1),
                        is_def: true,
                    });
                }
                break;
            }
        }
    }
    symbols
}

fn rust_symbol_kinds() -> Vec<(&'static str, SymbolKind)> {
    vec![
        ("fn ", SymbolKind::Function),
        ("struct ", SymbolKind::Struct),
        ("enum ", SymbolKind::Enum),
        ("trait ", SymbolKind::Trait),
        ("impl ", SymbolKind::Type),
        ("mod ", SymbolKind::Module),
        ("type ", SymbolKind::Type),
        ("const ", SymbolKind::Constant),
        ("static ", SymbolKind::Constant),
    ]
}

fn js_symbol_kinds() -> Vec<(&'static str, SymbolKind)> {
    vec![
        ("function ", SymbolKind::Function),
        ("async function ", SymbolKind::Function),
        ("class ", SymbolKind::Struct),
        ("const ", SymbolKind::Constant),
        ("let ", SymbolKind::Constant),
        ("var ", SymbolKind::Constant),
    ]
}

fn go_symbol_kinds() -> Vec<(&'static str, SymbolKind)> {
    vec![
        ("func ", SymbolKind::Function),
        ("type ", SymbolKind::Struct),
        ("const ", SymbolKind::Constant),
        ("var ", SymbolKind::Constant),
    ]
}

fn python_symbol_kinds() -> Vec<(&'static str, SymbolKind)> {
    vec![
        ("def ", SymbolKind::Function),
        ("class ", SymbolKind::Struct),
        ("async def ", SymbolKind::Function),
    ]
}
