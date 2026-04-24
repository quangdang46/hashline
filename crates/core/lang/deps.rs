use serde::Serialize;
use std::path::{Path, PathBuf};

/// Kind of import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ImportKind {
    Direct,
    Wildcard,
    SelfImport,
    Module,
}

/// A single import entry.
#[derive(Debug, Clone, Serialize)]
pub struct ImportEntry {
    pub kind: ImportKind,
    pub path: String,
    pub alias: Option<String>,
    pub line: usize,
}

impl ImportEntry {
    pub fn new(path: &str, line: usize) -> Self {
        Self {
            kind: ImportKind::Direct,
            path: path.to_string(),
            alias: None,
            line,
        }
    }

    pub fn with_alias(mut self, alias: &str) -> Self {
        self.alias = Some(alias.to_string());
        self
    }

    pub fn with_kind(mut self, kind: ImportKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn kind_debug(&self) -> &'static str {
        match self.kind {
            ImportKind::Direct => "import",
            ImportKind::Wildcard => "import *",
            ImportKind::SelfImport => "self",
            ImportKind::Module => "module",
        }
    }
}

/// Dependency analysis result.
#[derive(Debug, Clone, Serialize)]
pub struct DepsResult {
    pub file: String,
    pub imports: Vec<ImportEntry>,
    pub imported_by: Vec<PathBuf>,
}

impl DepsResult {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_string(),
            imports: Vec::new(),
            imported_by: Vec::new(),
        }
    }
}

/// Extract imports from source text, parsed per language.
pub fn extract_imports(
    text: &str,
    lang: crate::lang::detect::Lang,
    _path: &Path,
) -> Vec<ImportEntry> {
    let mut imports = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    match lang {
        crate::lang::detect::Lang::Rust => {
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("use ") {
                    let rest = trimmed.strip_prefix("use ").unwrap().trim_end_matches(';');
                    let alias = if rest.contains(" as ") {
                        rest.split(" as ").nth(1).map(|s| s.trim().to_string())
                    } else {
                        None
                    };
                    let path_str = rest.split(" as ").next().unwrap_or(rest).trim();
                    imports.push(ImportEntry {
                        kind: ImportKind::Direct,
                        path: path_str.to_string(),
                        alias,
                        line: i + 1,
                    });
                }
            }
        }
        crate::lang::detect::Lang::JavaScript | crate::lang::detect::Lang::TypeScript => {
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    if trimmed.contains("from ") {
                        if let Some(path_str) = trimmed.split("from ").nth(1) {
                            let clean = path_str
                                .trim()
                                .trim_matches(|c| c == '\'' || c == '"' || c == ';');
                            imports.push(ImportEntry {
                                kind: ImportKind::Direct,
                                path: clean.to_string(),
                                alias: None,
                                line: i + 1,
                            });
                        }
                    } else if trimmed.contains("* as ") {
                        if let Some(name) = trimmed.split("* as ").nth(1) {
                            let clean = name.trim().trim_end_matches(';');
                            imports.push(ImportEntry {
                                kind: ImportKind::Module,
                                path: clean.to_string(),
                                alias: None,
                                line: i + 1,
                            });
                        }
                    } else if trimmed.contains("{ ") {
                        imports.push(ImportEntry {
                            kind: ImportKind::Direct,
                            path: trimmed.to_string(),
                            alias: None,
                            line: i + 1,
                        });
                    }
                } else if trimmed.starts_with("require(") {
                    if let Some(path_str) = trimmed.split("require(").nth(1) {
                        let clean = path_str
                            .split(')')
                            .next()
                            .unwrap_or(path_str)
                            .trim()
                            .trim_matches(|c| c == '\'' || c == '"');
                        imports.push(ImportEntry {
                            kind: ImportKind::Direct,
                            path: clean.to_string(),
                            alias: None,
                            line: i + 1,
                        });
                    }
                }
            }
        }
        crate::lang::detect::Lang::Go => {
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    let rest = trimmed.strip_prefix("import ").unwrap().trim();
                    if rest.starts_with('"') {
                        let clean = rest.trim_matches('"');
                        imports.push(ImportEntry {
                            kind: ImportKind::Direct,
                            path: clean.to_string(),
                            alias: None,
                            line: i + 1,
                        });
                    }
                }
            }
        }
        crate::lang::detect::Lang::Python => {
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    let rest = trimmed
                        .strip_prefix("import ")
                        .unwrap()
                        .trim_end_matches(';')
                        .trim();
                    imports.push(ImportEntry {
                        kind: ImportKind::Direct,
                        path: rest.to_string(),
                        alias: None,
                        line: i + 1,
                    });
                } else if trimmed.starts_with("from ") {
                    if let Some(rest) = trimmed.split(" from ").nth(1) {
                        let clean = rest.trim_end_matches(';').trim();
                        imports.push(ImportEntry {
                            kind: ImportKind::Direct,
                            path: clean.to_string(),
                            alias: None,
                            line: i + 1,
                        });
                    }
                }
            }
        }
        _ => {}
    }
    imports
}
