"""Base task class for linehash benchmarks."""

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
import subprocess
from pathlib import Path


@dataclass
class Mutation:
    """A single file mutation: replace `original` with `mutated`."""
    file_path: str
    original: str
    mutated: str


@dataclass
class GroundTruth:
    """Expected elements for correctness validation."""
    required_strings: list[str] = field(default_factory=list)
    forbidden_strings: list[str] = field(default_factory=lambda: [
        "I cannot", "I don't have access", "no such file",
    ])
    file_path: str = ""
    expected_diff_contains: list[str] = field(default_factory=list)


class Task(ABC):
    @property
    @abstractmethod
    def name(self) -> str: ...

    @property
    @abstractmethod
    def prompt(self) -> str: ...

    @property
    @abstractmethod
    def ground_truth(self) -> GroundTruth: ...

    @property
    def task_type(self) -> str:
        return "read"

    @property
    def repo(self) -> str:
        return "synthetic"

    @property
    def mutations(self) -> list[Mutation]:
        return []

    def apply_mutations(self, repo_path: str) -> None:
        """Apply all mutations to the repo and commit them."""
        for m in self.mutations:
            fp = Path(repo_path) / m.file_path
            content = fp.read_text()
            if m.original not in content:
                raise ValueError(
                    f"Mutation target not found in {m.file_path}: "
                    f"{m.original[:80]!r}"
                )
            content = content.replace(m.original, m.mutated, 1)
            fp.write_text(content)

        mutated_files = [m.file_path for m in self.mutations]
        git_env = {
            "GIT_AUTHOR_NAME": "dev",
            "GIT_AUTHOR_EMAIL": "dev@test.com",
            "GIT_COMMITTER_NAME": "dev",
            "GIT_COMMITTER_EMAIL": "dev@test.com",
        }
        env = {**os.environ, **git_env}
        subprocess.run(
            ["git", "add"] + mutated_files,
            cwd=repo_path, check=True, capture_output=True, env=env,
        )
        subprocess.run(
            ["git", "commit", "-m", "refactor: simplify edge case handling"],
            cwd=repo_path, check=True, capture_output=True, env=env,
        )

    def check_correctness(self, result_text: str, repo_path: str) -> tuple[bool, str]:
        """Validate result against ground truth."""
        gt = self.ground_truth

        # Mutation tasks with a test command: run the test
        if self.mutations and self.test_command:
            result = subprocess.run(
                self.test_command,
                cwd=repo_path, capture_output=True, text=True,
                timeout=300,
            )
            if result.returncode != 0:
                return False, f"Test failed: {self.test_command[-1]}"
            return True, "Test passed"

        # Forward-edit tasks: check git diff for expected patterns
        diff = ""
        if self.task_type == "edit" and gt.file_path:
            result = subprocess.run(
                ["git", "diff", gt.file_path],
                cwd=repo_path, capture_output=True, text=True,
            )
            diff = result.stdout
            if not self.mutations:
                if not diff:
                    return False, "No changes in target file"
                for pattern in gt.expected_diff_contains:
                    if pattern not in diff:
                        return False, f"Diff missing: {pattern}"

        # Read tasks / forward-edit tasks: check required_strings in response + diff
        combined = (result_text + "\n" + diff).replace("`", "")
        text_lower = combined.lower()

        for required in gt.required_strings:
            if required.lower() not in text_lower:
                return False, f"Missing: {required}"

        for forbidden in gt.forbidden_strings:
            if forbidden.lower() in text_lower:
                return False, f"Contains forbidden: {forbidden}"

        return True, "All checks passed"

    @property
    def test_command(self) -> list[str]:
        return []
