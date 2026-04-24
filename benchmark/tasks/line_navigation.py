"""Line navigation task - find function definitions in a Rust file."""

from .base import Task, GroundTruth


class LineNavigationTask(Task):
    """Task that tests whether AI can correctly identify function definitions with line numbers."""

    @property
    def name(self) -> str:
        return "line_navigation"

    @property
    def prompt(self) -> str:
        return (
            "Find all function definitions in test_fixtures.rs and list them with their line numbers. "
            "The file test_fixtures.rs is in the current directory. "
            "Format your answer as: 'fn <name> at line <N>' for each function found."
        )

    @property
    def ground_truth(self) -> GroundTruth:
        return GroundTruth(
            required_strings=[
                "test_fixtures.rs",
                "fn helper_function",
                "line",
                "fn calculate",
                "fn validate",
            ]
        )

    @property
    def task_type(self) -> str:
        return "read"
