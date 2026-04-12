#![allow(dead_code)]

pub fn generate_short_fixture(line_count: usize) -> String {
    let mut lines = Vec::with_capacity(line_count);
    let marker_line = line_count / 10;
    for i in 0..line_count {
        let marker = if i == marker_line {
            " SPARSE_MARKER_12345"
        } else {
            ""
        };
        lines.push(format!(
            "fn generated_line_{i:05}() {{ let value = \"{:08x}\";{marker} }}",
            i.wrapping_mul(2654435761_u32 as usize)
        ));
    }
    lines.join("\n") + "\n"
}

pub fn generate_long_fixture(line_count: usize) -> String {
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        lines.push(format!(
            "pub fn generated_line_{i:05}(input: &str) -> String {{ let value = format!(\"{}::{}::{}\", input, {i}, \"benchmark_payload_{:08x}\"); value.trim().to_owned() }}",
            "segment",
            "payload",
            "suffix",
            i.wrapping_mul(11400714819323198485_u64 as usize)
        ));
    }
    lines.join("\n") + "\n"
}

pub fn generate_collision_fixture(
    line_count: usize,
    mut short_hash: impl FnMut(&str) -> String,
) -> String {
    let (first, second) = find_collision_pair(&mut short_hash);
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
    lines.join("\n") + "\n"
}

pub fn mutate_short_hash(short: &str) -> String {
    let mut chars = short.chars();
    let first = chars.next().unwrap_or('0');
    let second = chars.next().unwrap_or('0');
    let replacement = if first == '0' { '1' } else { '0' };
    format!("{replacement}{second}")
}

#[derive(Clone, Debug)]
pub struct EditScenario {
    pub original_content: String,
    pub drifted_content: String,
    pub target_line_number: usize,
    pub target_anchor: String,
    pub replacement_line: String,
    pub expected_target_line: String,
    pub naive_old_line: String,
    pub naive_new_line: String,
    pub naive_old_block: String,
    pub naive_new_block: String,
}

pub type WhitespaceDriftEditScenario = EditScenario;

pub fn generate_exact_match_edit_scenario(line_count: usize) -> EditScenario {
    let mut scenario = build_base_edit_scenario(line_count, false);
    scenario.drifted_content = scenario.original_content.clone();
    scenario
}

pub fn generate_whitespace_drift_edit_scenario(line_count: usize) -> WhitespaceDriftEditScenario {
    let mut scenario = build_base_edit_scenario(line_count, false);

    let mut drifted_lines = split_lines(&scenario.original_content);
    let drift_index = scenario.target_line_number - 2;
    drifted_lines[drift_index] = "  let surrounding_context = compute_timeout_window();".to_owned();
    scenario.drifted_content = drifted_lines.join("\n") + "\n";
    scenario.naive_new_block = [
        drifted_lines[drift_index].as_str(),
        scenario.replacement_line.as_str(),
        drifted_lines[scenario.target_line_number].as_str(),
    ]
    .join("\n");
    scenario
}

pub fn generate_target_whitespace_drift_edit_scenario(line_count: usize) -> EditScenario {
    let mut scenario = build_base_edit_scenario(line_count, false);

    let mut drifted_lines = split_lines(&scenario.original_content);
    let target_index = scenario.target_line_number - 1;
    drifted_lines[target_index] = "  timeout: 3000,".to_owned();
    scenario.drifted_content = drifted_lines.join("\n") + "\n";
    scenario
}

pub fn generate_duplicate_target_edit_scenario(line_count: usize) -> EditScenario {
    let mut scenario = build_base_edit_scenario(line_count, false);

    let mut lines = split_lines(&scenario.original_content);
    let target_index = scenario.target_line_number - 1;
    let duplicate_index = target_index - 3;
    lines[duplicate_index] = scenario.naive_old_line.clone();
    scenario.original_content = lines.join("\n") + "\n";
    scenario.drifted_content = scenario.original_content.clone();
    scenario
}

pub fn generate_long_line_exact_match_edit_scenario(line_count: usize) -> EditScenario {
    let mut scenario = build_base_edit_scenario(line_count, true);
    scenario.drifted_content = scenario.original_content.clone();
    scenario
}

pub fn generate_line_shift_edit_scenario(line_count: usize) -> EditScenario {
    let mut scenario = build_base_edit_scenario(line_count, false);
    let mut drifted_lines = split_lines(&scenario.original_content);
    let target_index = scenario.target_line_number - 1;
    drifted_lines.insert(
        target_index - 1,
        "fn inserted_line_before_target() { let marker = \"line_shift\"; }".to_owned(),
    );
    scenario.drifted_content = drifted_lines.join("\n") + "\n";
    scenario
}

fn build_base_edit_scenario(line_count: usize, long_lines: bool) -> EditScenario {
    assert!(line_count >= 5, "line_count must be at least 5");

    let target_index = line_count / 2;
    let drift_index = target_index - 1;
    let after_index = target_index + 1;

    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        let line = if long_lines {
            format!(
                "pub fn generated_line_{i:05}(input: &str) -> String {{ let value = format!(\"{}::{}::{}\", input, {i}, \"benchmark_payload_{:08x}\"); value.trim().to_owned() }}",
                "segment",
                "payload",
                "suffix",
                i.wrapping_mul(11400714819323198485_u64 as usize)
            )
        } else {
            format!(
                "fn generated_line_{i:05}() {{ let value = \"{:08x}\"; }}",
                i.wrapping_mul(2654435761_u32 as usize)
            )
        };
        lines.push(line);
    }

    if long_lines {
        lines[drift_index] = "    let surrounding_context = compute_timeout_window_with_extended_payload(segment, payload, suffix, 42, \"long_line_context\");".to_owned();
        lines[target_index] =
            "    timeout: 3000, // benchmark_payload_long_form_with_additional_context_tokens"
                .to_owned();
        lines[after_index] =
            "    retry: true, // benchmark_payload_long_form_followup_context".to_owned();
    } else {
        lines[drift_index] = "    let surrounding_context = compute_timeout_window();".to_owned();
        lines[target_index] = "    timeout: 3000,".to_owned();
        lines[after_index] = "    retry: true,".to_owned();
    }

    let target_line_number = target_index + 1;
    let target_line = lines[target_index].clone();
    let replacement_line = if long_lines {
        "    timeout: 5000, // benchmark_payload_long_form_with_additional_context_tokens"
            .to_owned()
    } else {
        "    timeout: 5000,".to_owned()
    };

    EditScenario {
        original_content: lines.join("\n") + "\n",
        drifted_content: String::new(),
        target_line_number,
        target_anchor: format!(
            "{}:{}",
            target_line_number,
            crate::hash::short_hash(&target_line)
        ),
        replacement_line: replacement_line.clone(),
        expected_target_line: replacement_line.clone(),
        naive_old_line: target_line.clone(),
        naive_new_line: replacement_line,
        naive_old_block: [
            lines[drift_index].as_str(),
            lines[target_index].as_str(),
            lines[after_index].as_str(),
        ]
        .join("\n"),
        naive_new_block: [
            lines[drift_index].as_str(),
            "    timeout: 5000,",
            lines[after_index].as_str(),
        ]
        .join("\n"),
    }
}

fn split_lines(content: &str) -> Vec<String> {
    content.lines().map(|line| line.to_owned()).collect()
}

fn find_collision_pair(short_hash: &mut impl FnMut(&str) -> String) -> (String, String) {
    use std::collections::HashMap;

    let mut seen: HashMap<String, String> = HashMap::new();
    for i in 0..10_000 {
        let candidate = format!("line-{i}");
        let hash = short_hash(&candidate);
        if let Some(existing) = seen.insert(hash, candidate.clone()) {
            if existing != candidate {
                return (existing, candidate);
            }
        }
    }
    panic!("failed to find a short-hash collision in search space");
}
