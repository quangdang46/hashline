#!/usr/bin/env python3
"""
Synthetic Rust project generator for linehash benchmarks.

Generates a deterministic Rust source file with specific
ground truth strings embedded for correctness checking.
"""

import subprocess
from pathlib import Path


REPO_PATH = Path(__file__).parent / "repo"


def get_test_fixtures_content() -> str:
    """Generate test_fixtures.rs with known function definitions."""
    return '''//! Test fixtures for linehash benchmark.
//!
//! This file contains known function definitions for testing
//! whether AI agents can correctly identify and locate functions.

#![allow(dead_code)]
#![allow(unused_variables)]

/// Helper function that performs basic arithmetic.
/// Returns the sum of two numbers.
fn helper_function(a: i32, b: i32) -> i32 {
    a + b
}

/// Calculate the factorial of a number recursively.
/// Returns None for negative inputs.
fn calculate_factorial(n: u32) -> Option<u64> {
    if n == 0 {
        Some(1)
    } else {
        match calculate_factorial(n - 1) {
            Some(result) => Some(n as u64 * result),
            None => None,
        }
    }
}

/// Validate that a string matches a given pattern.
/// Returns true if valid, false otherwise.
fn validate_string(input: &str, pattern: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    if pattern.is_empty() {
        return true;
    }
    input.len() == pattern.len()
}

/// Process a list of items and return their string representations.
fn process_items(items: Vec<i32>) -> Vec<String> {
    items.iter().map(|i| i.to_string()).collect()
}

/// A struct representing a configuration with validation.
pub struct Config {
    pub name: String,
    pub enabled: bool,
    pub timeout_ms: u64,
}

impl Config {
    /// Create a new Config with default values.
    pub fn new() -> Self {
        Config {
            name: String::new(),
            enabled: false,
            timeout_ms: 1000,
        }
    }

    /// Validate the configuration settings.
    pub fn is_valid(&self) -> bool {
        self.timeout_ms > 0 && !self.name.is_empty()
    }
}

/// Represents a result that can be successful or an error.
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> Result<T, E> {
    /// Returns true if this is an Ok result.
    pub fn is_ok(&self) -> bool {
        match self {
            Result::Ok(_) => true,
            Result::Err(_) => false,
        }
    }

    /// Returns the value if Ok, or a default otherwise.
    pub fn unwrap_or(&self, default: T) -> T {
        match self {
            Result::Ok(v) => v.clone(),
            Result::Err(_) => default,
        }
    }
}

/// Main entry point for testing (empty in library code).
pub fn main() {
    println!("Test fixtures loaded");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helper_function() {
        assert_eq!(helper_function(2, 3), 5);
    }

    #[test]
    fn test_factorial() {
        assert_eq!(calculate_factorial(5), Some(120));
    }

    #[test]
    fn test_validate_string() {
        assert!(validate_string("hello", "hello"));
        assert!(!validate_string("hello", "world"));
    }
}
'''


def setup_repo():
    """Set up the synthetic repository."""
    # Create directory structure
    print(f"Creating repo at {REPO_PATH}")
    REPO_PATH.mkdir(parents=True, exist_ok=True)

    # Write the test fixtures file
    test_fixtures = REPO_PATH / "test_fixtures.rs"
    test_fixtures.write_text(get_test_fixtures_content())
    print(f"  Created test_fixtures.rs ({len(get_test_fixtures_content().splitlines())} lines)")

    # Initialize git repo
    print("\nInitializing git repository...")
    subprocess.run(["git", "init"], cwd=REPO_PATH, check=True, capture_output=True)
    subprocess.run(["git", "add", "."], cwd=REPO_PATH, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "Initial commit"],
        cwd=REPO_PATH,
        check=True,
        capture_output=True
    )

    print("\n" + "="*60)
    print("Repository setup complete!")
    print(f"Location: {REPO_PATH}")
    print("="*60)


def reset_repo():
    """Reset the repo to clean state."""
    subprocess.run(["git", "checkout", "--", "."], cwd=REPO_PATH, check=True, capture_output=True)
    subprocess.run(["git", "clean", "-fd"], cwd=REPO_PATH, check=True, capture_output=True)


if __name__ == "__main__":
    setup_repo()
