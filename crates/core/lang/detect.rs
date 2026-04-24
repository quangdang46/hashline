use std::path::Path;

/// Language detection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Go,
    C,
    Cpp,
    Java,
    Html,
    Css,
    Json,
    Toml,
    Markdown,
    Yaml,
    PlainText,
}

impl Lang {
    pub fn from_path(path: &Path) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext.to_lowercase().as_str() {
            "rs" => Lang::Rust,
            "js" | "mjs" | "cjs" => Lang::JavaScript,
            "ts" | "mts" | "cts" => Lang::TypeScript,
            "jsx" | "tsx" => Lang::JavaScript,
            "py" | "pyw" => Lang::Python,
            "go" => Lang::Go,
            "c" | "h" => Lang::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hxx" => Lang::Cpp,
            "java" => Lang::Java,
            "html" | "htm" => Lang::Html,
            "css" | "scss" | "sass" | "less" => Lang::Css,
            "json" => Lang::Json,
            "toml" => Lang::Toml,
            "md" | "markdown" => Lang::Markdown,
            "yaml" | "yml" => Lang::Yaml,
            _ => Lang::PlainText,
        }
    }

    pub fn is_source(&self) -> bool {
        !matches!(
            self,
            Lang::PlainText | Lang::Json | Lang::Toml | Lang::Markdown | Lang::Yaml
        )
    }
}

/// Detect language from a path (free function).
pub fn detect_language_from_path(path: &Path) -> Lang {
    Lang::from_path(path)
}
