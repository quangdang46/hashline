"""Edit verification task - add a function at a specific line."""

from .base import Task, GroundTruth


class EditVerificationTask(Task):
    """Task that tests whether AI can correctly add a function at a specific line."""

    @property
    def name(self) -> str:
        return "edit_verification"

    @property
    def prompt(self) -> str:
        return (
            "In test_fixtures.rs, add a new public function `fn test_func()` that returns `()` at line 5. "
            "The function body should be empty (just `{}`)."
        )

    @property
    def ground_truth(self) -> GroundTruth:
        return GroundTruth(
            required_strings=["test_func"],
            file_path="test_fixtures.rs",
            expected_diff_contains=["pub fn test_func()"],
        )

    @property
    def task_type(self) -> str:
        return "edit"
