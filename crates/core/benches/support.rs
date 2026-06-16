#![allow(dead_code)]

use std::path::PathBuf;

use hashline::document::FileContent;
use hashline::hash;

pub fn make_fc(content: &str) -> FileContent {
    FileContent {
        path: PathBuf::from("bench.rs"),
        raw: content.to_string(),
        normalized: content.to_string(),
        newline: hashline::document::NewlineStyle::Lf,
        trailing_newline: content.ends_with('\n'),
        hash: "0000".into(),
    }
}

pub fn generate_short_fixture(line_count: usize) -> FileContent {
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        lines.push(format!(
            "fn generated_line_{i:05}() {{ let value = \"{:08x}\"; }}",
            i.wrapping_mul(2654435761_u32 as usize)
        ));
    }
    make_fc(&(lines.join("\n") + "\n"))
}

pub fn generate_long_fixture(line_count: usize) -> FileContent {
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        lines.push(format!(
            "pub fn generated_line_{i:05}(input: &str) -> String {{ let value = format!(\"{}::{}::{}\", input, {i}, \"benchmark_payload_{:08x}\"); value.trim().to_owned() }}",
            "segment", "payload", "suffix",
            i.wrapping_mul(11400714819323198485_u64 as usize)
        ));
    }
    make_fc(&(lines.join("\n") + "\n"))
}

pub fn generate_collision_fixture(line_count: usize) -> FileContent {
    let (first, second) = find_collision_pair();
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        if i % 16 == 0 {
            lines.push(first.clone());
        } else if i % 16 == 1 {
            lines.push(second.clone());
        } else {
            lines.push(format!(
                "unique-line-{i:05}-{:08x}",
                i.wrapping_mul(1103515245)
            ));
        }
    }
    make_fc(&(lines.join("\n") + "\n"))
}

#[derive(Clone, Debug)]
pub struct EditScenario {
    pub content: String,
    pub target_line_number: usize,
    pub target_anchor: String,
    pub replacement_line: String,
    pub expected_target_line: String,
}

pub fn generate_exact_match_edit_scenario(line_count: usize) -> EditScenario {
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        lines.push(format!(
            "fn generated_line_{i:05}() {{ let value = \"{:08x}\"; }}",
            i.wrapping_mul(2654435761_u32 as usize)
        ));
    }

    let target_index = line_count / 2;
    let target_line_number = target_index + 1;
    let content = lines.join("\n") + "\n";
    let fc = make_fc(&content);
    let entries = fc.lines_with_hashes();
    let target_hash = hash::format_short_hash(entries[target_index].short_hash);

    EditScenario {
        content,
        target_line_number,
        target_anchor: format!("{target_line_number}:{target_hash}"),
        replacement_line: "    timeout: 5000,".to_owned(),
        expected_target_line: "    timeout: 5000,".to_owned(),
    }
}

fn find_collision_pair() -> (String, String) {
    use std::collections::HashMap;
    let mut seen: HashMap<u8, String> = HashMap::new();
    for i in 0..10_000 {
        let candidate = format!("line-{i}");
        let hash_val = hash::short_hash_value(&candidate);
        if let Some(existing) = seen.insert(hash_val, candidate.clone()) {
            if existing != candidate {
                return (existing, candidate);
            }
        }
    }
    panic!("failed to find a short-hash collision in search space");
}
