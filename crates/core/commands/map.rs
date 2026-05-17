use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::Serialize;

use crate::cli::MapCmd;
use crate::context::CommandContext;
use crate::error::LinehashError;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: MapCmd,
) -> Result<(), LinehashError> {
    let scope = cmd
        .scope
        .as_deref()
        .or(cmd.path.as_deref())
        .unwrap_or_else(|| Path::new("."));
    let depth = cmd.depth.unwrap_or(usize::MAX);
    let budget = cmd.budget;

    let tree = generate_map(scope, depth, budget)?;

    if cmd.json {
        output::write_json_success(ctx, &tree)?;
    } else {
        let summary = format!(
            "{} files, {} tokens, {} truncated",
            tree.total_files, tree.total_tokens, tree.truncated
        );
        output::write_success_line(ctx, &summary)?;
    }

    Ok(())
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub path: PathBuf,
    pub token_count: usize,
    pub symbols: Vec<String>,
    pub file_type: String,
}

#[derive(Debug, Serialize)]
pub struct MapNode {
    pub path: PathBuf,
    pub token_count: usize,
    pub files: Vec<FileEntry>,
    pub subdirs: BTreeMap<String, MapNode>,
}

#[derive(Debug, Serialize)]
pub struct MapResult {
    pub root: String,
    pub total_tokens: usize,
    pub total_files: usize,
    pub tree: BTreeMap<String, MapNode>,
    pub truncated: bool,
}

pub fn generate_map(
    scope: &Path,
    depth: usize,
    budget: Option<u64>,
) -> Result<MapResult, LinehashError> {
    let mut tree: BTreeMap<String, MapNode> = BTreeMap::new();
    let mut total_tokens = 0usize;
    let mut total_files = 0usize;
    let mut truncated = false;

    let walker = WalkBuilder::new(scope)
        .git_ignore(true)
        .hidden(true)
        .max_depth(Some(depth))
        .build();

    for entry in walker.flatten() {
        let path = entry.path().to_path_buf();
        if !path.is_file() {
            continue;
        }

        let file_type = detect_file_type(&path);
        if file_type == "unknown" {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let file_size = metadata.len() as usize;

        let token_count = estimate_tokens(&path, file_size);
        total_tokens += token_count;
        total_files += 1;

        if let Some(budget) = budget {
            if total_tokens as u64 > budget {
                truncated = true;
                break;
            }
        }

        insert_into_tree(&mut tree, &path, token_count, &file_type);
    }

    rollup_tokens(&mut tree);

    Ok(MapResult {
        root: scope.display().to_string(),
        total_tokens,
        total_files,
        tree,
        truncated,
    })
}

fn detect_file_type(path: &Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => "rust".to_string(),
        "js" | "mjs" | "cjs" => "javascript".to_string(),
        "ts" | "mts" | "cts" => "typescript".to_string(),
        "jsx" | "tsx" => "tsx".to_string(),
        "py" | "pyw" => "python".to_string(),
        "go" => "go".to_string(),
        "java" => "java".to_string(),
        "c" | "h" => "c".to_string(),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "cpp".to_string(),
        "rb" => "ruby".to_string(),
        "php" => "php".to_string(),
        "cs" => "csharp".to_string(),
        "swift" => "swift".to_string(),
        "scala" => "scala".to_string(),
        "kt" | "kts" => "kotlin".to_string(),
        "ex" | "exs" => "elixir".to_string(),
        "md" | "markdown" => "markdown".to_string(),
        "toml" | "yaml" | "yml" | "json" => "config".to_string(),
        _ => "unknown".to_string(),
    }
}

fn estimate_tokens(path: &Path, file_size: usize) -> usize {
    let base_tokens = file_size / 4;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "md" | "markdown" => file_size / 3,
        "json" | "yaml" | "yml" | "toml" => file_size / 5,
        _ => base_tokens,
    }
}

fn insert_into_tree(
    tree: &mut BTreeMap<String, MapNode>,
    path: &Path,
    token_count: usize,
    file_type: &str,
) {
    let relative = path
        .strip_prefix(
            path.components()
                .next()
                .map(|c| c.as_os_str())
                .unwrap_or(path.as_os_str()),
        )
        .unwrap_or(path);

    let parts: Vec<String> = relative
        .parent()
        .map(|p| {
            p.iter()
                .filter_map(|c| c.to_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let key = if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    };

    let file_entry = FileEntry {
        path: path.to_path_buf(),
        token_count,
        symbols: Vec::new(),
        file_type: file_type.to_string(),
    };

    tree.entry(key.clone())
        .or_insert_with(|| MapNode {
            path: PathBuf::from(&key),
            token_count: 0,
            files: Vec::new(),
            subdirs: BTreeMap::new(),
        })
        .files
        .push(file_entry);
}

fn rollup_tokens(tree: &mut BTreeMap<String, MapNode>) {
    for node in tree.values_mut() {
        for subdir in node.subdirs.values_mut() {
            rollup_single_node(subdir);
        }
    }
}

fn rollup_single_node(node: &mut MapNode) {
    let mut total = 0usize;

    for file in &node.files {
        total += file.token_count;
    }

    for subdir in node.subdirs.values_mut() {
        rollup_single_node(subdir);
        total += subdir.token_count;
    }

    node.token_count = total;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_file_type() {
        assert_eq!(detect_file_type(Path::new("foo.rs")), "rust");
        assert_eq!(detect_file_type(Path::new("bar.js")), "javascript");
        assert_eq!(detect_file_type(Path::new("baz.ts")), "typescript");
        assert_eq!(detect_file_type(Path::new("qux.py")), "python");
        assert_eq!(detect_file_type(Path::new("unknown.xyz")), "unknown");
    }

    #[test]
    fn test_estimate_tokens() {
        let tokens = estimate_tokens(Path::new("test.rs"), 100);
        assert!((20..=30).contains(&tokens));
    }
}
