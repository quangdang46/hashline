use serde::Serialize;
use std::path::Path;

use crate::lang::detect::Lang;
use crate::lang::parser_pool;

/// Maximum number of hubs before switching to parallel mode.
pub const AUTO_HUB_THRESHOLD: usize = 200;

/// Ratio of suspicious nodes to trigger suspicion tracking.
pub const SUSPICION_RATIO: f64 = 0.3;

/// A call graph edge (caller -> callee).
#[derive(Debug, Clone, Serialize)]
pub struct CallEdge {
    pub from: String,
    pub from_file: String,
    pub from_line: usize,
    pub to: String,
}

/// Call graph search result.
#[derive(Debug, Clone, Serialize)]
pub struct CallGraphResult {
    pub target: String,
    pub depth: usize,
    pub edges: Vec<CallEdge>,
    pub visited: usize,
}

impl CallGraphResult {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_string(),
            depth: 0,
            edges: Vec::new(),
            visited: 0,
        }
    }
}

/// A function definition found by tree-sitter.
#[derive(Debug, Clone)]
struct FuncDef {
    name: String,
    start_line: usize,
    end_line: usize,
}

/// BFS search for callers (functions that CALL the given target) using tree-sitter.
pub fn search_callers_bfs(target: &str, scope: &Path, _depth: usize) -> CallGraphResult {
    let mut result = CallGraphResult::new(target);
    let mut result_set = std::collections::HashSet::new();

    let walker = ignore::WalkBuilder::new(scope)
        .hidden(true)
        .git_ignore(true)
        .build();

    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let lang = Lang::from_path(path);
        if !lang.is_source() {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file_str = path.to_string_lossy().to_string();

        // Use tree-sitter if available (parser is cached per-thread via parser_pool).
        let tree = parser_pool::with_parser(lang, |parser| parser.parse(&content, None)).flatten();
        if let Some(tree) = tree {
            let funcs = extract_function_definitions(tree.root_node());
            let calls = find_call_sites(tree.root_node(), target);

            for call in calls {
                // Find which function this call belongs to
                if let Some(enclosing) = find_enclosing_func(call.line, &funcs) {
                    let key = format!("{}:{}", enclosing.name, enclosing.start_line);
                    if !result_set.contains(&key) {
                        result_set.insert(key);
                        result.edges.push(CallEdge {
                            from: enclosing.name,
                            from_file: file_str.clone(),
                            from_line: enclosing.start_line,
                            to: target.to_string(),
                        });
                    }
                }
            }
            continue;
        }

        // Fallback to naive pattern matching
        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.contains(&format!("{}(", target))
                || trimmed.contains(&format!(".{}(", target))
            {
                if let Some((func_name, func_line)) =
                    find_enclosing_function_naive(&content, line_idx)
                {
                    let key = format!("{}:{}", func_name, func_line);
                    if !result_set.contains(&key) {
                        result_set.insert(key);
                        result.edges.push(CallEdge {
                            from: func_name,
                            from_file: file_str.clone(),
                            from_line: func_line,
                            to: target.to_string(),
                        });
                    }
                }
            }
        }
    }

    result.visited = result.edges.len();
    result
}

/// Extract all function definitions from the AST using tree-sitter.
fn extract_function_definitions(root: tree_sitter::Node) -> Vec<FuncDef> {
    let mut funcs = Vec::new();
    let mut cursor = root.walk();
    collect_fn_defs(root, &mut cursor, 0, &mut funcs);
    funcs
}

fn collect_fn_defs(
    node: tree_sitter::Node,
    _cursor: &mut tree_sitter::TreeCursor,
    _depth: usize,
    funcs: &mut Vec<FuncDef>,
) {
    let kind = node.kind();

    let name_opt = match kind {
        "function_item" | "function_declaration" | "function_definition" => node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("identifier"))
            .map(|n| node_text(n)),
        "method_definition" => node.child_by_field_name("name").map(|n| node_text(n)),
        _ => None,
    };

    if let Some(name) = name_opt {
        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        funcs.push(FuncDef {
            name,
            start_line,
            end_line,
        });
    }

    // Search children for nested functions
    for child in node.children(&mut node.walk()) {
        collect_fn_defs(child, &mut node.walk(), _depth + 1, funcs);
    }
}

/// A call site found by tree-sitter.
#[derive(Debug)]
struct CallSite {
    name: String,
    line: usize,
}

/// Find all call sites that reference the target function using tree-sitter.
fn find_call_sites(root: tree_sitter::Node, target: &str) -> Vec<CallSite> {
    let mut calls = Vec::new();
    let mut cursor = root.walk();
    find_calls(root, &mut cursor, target, &mut calls);
    calls
}

fn find_calls(
    node: tree_sitter::Node,
    _cursor: &mut tree_sitter::TreeCursor,
    target: &str,
    calls: &mut Vec<CallSite>,
) {
    let kind = node.kind();

    // Call expressions: call, method_call, identifier
    if kind == "call_expression" || kind == "method_call_expression" {
        let func_node = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("method"));

        if let Some(func) = func_node {
            let name = node_text(func);
            if name == target || name.contains(target) {
                calls.push(CallSite {
                    name,
                    line: node.start_position().row + 1,
                });
            }
        }
    } else if kind == "identifier" {
        let name = node_text(node);
        if name == target {
            calls.push(CallSite {
                name,
                line: node.start_position().row + 1,
            });
        }
    }

    // Recurse
    for child in node.children(&mut node.walk()) {
        find_calls(child, &mut node.walk(), target, calls);
    }
}

fn node_text(node: tree_sitter::Node) -> String {
    node.kind().to_string()
}

/// Find which function encloses a given line number.
fn find_enclosing_func(line: usize, funcs: &[FuncDef]) -> Option<FuncDef> {
    funcs
        .iter()
        .filter(|f| line >= f.start_line && line <= f.end_line)
        .min_by_key(|f| f.end_line - f.start_line)
        .cloned()
}

/// BFS search for callees (functions called BY the given target) using tree-sitter.
pub fn search_callees_bfs(target: &str, scope: &Path, depth: usize) -> CallGraphResult {
    let mut result = CallGraphResult::new(target);
    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![target.to_string()];

    let mut current_depth = 0;
    while current_depth < depth && !queue.is_empty() {
        let mut next_queue = vec![];
        for sym in queue {
            if visited.contains(&sym) {
                continue;
            }
            visited.insert(sym.clone());

            let walker = ignore::WalkBuilder::new(scope)
                .hidden(true)
                .git_ignore(true)
                .build();

            for entry in walker.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let lang = Lang::from_path(path);
                if !lang.is_source() {
                    continue;
                }

                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let file_str = path.to_string_lossy().to_string();

                // Use tree-sitter if available (parser is cached per-thread via parser_pool).
                let tree =
                    parser_pool::with_parser(lang, |parser| parser.parse(&content, None)).flatten();
                if let Some(tree) = tree {
                    let funcs = extract_function_definitions(tree.root_node());

                    // Find the target function
                    if let Some(target_func) = funcs.iter().find(|f| f.name == sym) {
                        // Find all calls within the target function body
                        let calls = find_calls_in_function(
                            tree.root_node(),
                            target_func.start_line,
                            target_func.end_line,
                        );

                        for call in calls {
                            if !result
                                .edges
                                .iter()
                                .any(|e| e.from == *sym && e.to == call.name)
                            {
                                result.edges.push(CallEdge {
                                    from: sym.clone(),
                                    from_file: file_str.clone(),
                                    from_line: target_func.start_line,
                                    to: call.name.clone(),
                                });
                                if !visited.contains(&call.name) && current_depth + 1 < depth {
                                    next_queue.push(call.name.clone());
                                }
                            }
                        }
                    }
                    continue;
                }

                // Fallback to naive
                let lines: Vec<&str> = content.lines().collect();
                for (line_idx, line) in lines.iter().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") {
                        let name = trimmed
                            .split(|c: char| c.is_whitespace() || c == '(')
                            .nth(1)
                            .unwrap_or(trimmed);
                        if name == sym {
                            let func_end = find_function_end_naive(&content, line_idx);
                            let body = lines[line_idx..func_end.min(lines.len())].join("\n");

                            for call_line in body.lines() {
                                if let Some(call_name) = extract_function_call_naive(call_line) {
                                    if !result
                                        .edges
                                        .iter()
                                        .any(|e| e.from == *name && e.to == call_name)
                                    {
                                        result.edges.push(CallEdge {
                                            from: name.to_string(),
                                            from_file: file_str.clone(),
                                            from_line: line_idx + 1,
                                            to: call_name.clone(),
                                        });
                                        if !visited.contains(&call_name)
                                            && current_depth + 1 < depth
                                        {
                                            next_queue.push(call_name.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        queue = next_queue;
        current_depth += 1;
    }

    result.depth = current_depth;
    result.visited = visited.len();
    result
}

/// Find all call sites within a function's line range.
fn find_calls_in_function(
    root: tree_sitter::Node,
    start_line: usize,
    end_line: usize,
) -> Vec<CallSite> {
    let mut calls = Vec::new();
    let mut cursor = root.walk();
    find_calls_in_range(root, &mut cursor, start_line, end_line, &mut calls);
    calls
}

fn find_calls_in_range(
    node: tree_sitter::Node,
    _cursor: &mut tree_sitter::TreeCursor,
    start: usize,
    end: usize,
    calls: &mut Vec<CallSite>,
) {
    let node_start = node.start_position().row + 1;
    let node_end = node.end_position().row + 1;

    // Skip nodes outside our range
    if node_end < start || node_start > end {
        return;
    }

    let kind = node.kind();
    if kind == "call_expression" || kind == "method_call_expression" {
        let func_node = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("method"));

        if let Some(func) = func_node {
            let name = node_text(func);
            calls.push(CallSite {
                name,
                line: node.start_position().row + 1,
            });
        }
    }

    for child in node.children(&mut node.walk()) {
        find_calls_in_range(child, &mut node.walk(), start, end, calls);
    }
}

// ---------------------------------------------------------------------------
// Fallback: naive implementations when tree-sitter is unavailable
// ---------------------------------------------------------------------------

fn find_enclosing_function_naive(content: &str, line_idx: usize) -> Option<(String, usize)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut func_start_line: Option<usize> = None;
    let mut func_name = String::new();

    for (i, line) in lines.iter().enumerate() {
        if i > line_idx {
            break;
        }
        let trimmed = line.trim();

        if func_start_line.is_none()
            && (trimmed.starts_with("fn ") || trimmed.starts_with("pub fn "))
        {
            let name = trimmed
                .split(|c: char| c.is_whitespace() || c == '(')
                .nth(1)
                .unwrap_or(trimmed);
            func_name = name.to_string();
            func_start_line = Some(i);
        }

        if let Some(start) = func_start_line {
            if i >= start {
                if i == start {
                    continue;
                }
                let delta = line.matches('{').count() as i32 - line.matches('}').count() as i32;
                if delta != 0 || trimmed.contains('}') {
                    return Some((func_name, i + 1));
                }
            }
        }
    }
    None
}

fn find_function_end_naive(content: &str, start_line: usize) -> usize {
    let lines: Vec<&str> = content.lines().collect();
    let mut brace_count = 0;
    let mut found_start = false;

    for (i, line) in lines.iter().enumerate().skip(start_line) {
        brace_count += line.matches('{').count() as i32;
        brace_count -= line.matches('}').count() as i32;
        found_start = found_start || line.contains('{');
        if found_start && brace_count <= 0 {
            return i + 1;
        }
    }
    lines.len()
}

fn extract_function_call_naive(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.ends_with(';') || trimmed.ends_with('{') {
        return None;
    }
    if trimmed.contains("fn ") {
        return None;
    }
    for (i, _) in trimmed.match_indices('(') {
        let before = &trimmed[..i];
        let name = before
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .next_back()
            .unwrap_or("");
        if !name.is_empty() && name.len() > 1 {
            let keywords = [
                "if", "else", "for", "while", "match", "return", "let", "const", "mut", "impl",
                "trait", "struct", "enum",
            ];
            if !keywords.contains(&name) {
                return Some(name.to_string());
            }
        }
    }
    None
}
