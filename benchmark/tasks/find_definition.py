from .base import Task, GroundTruth


class FindDefinitionTask(Task):
    @property
    def name(self) -> str:
        return "find_definition"

    @property
    def prompt(self) -> str:
        return (
            "Find where `validate_jwt_token` is defined in this codebase. "
            "Show the full implementation including the function signature, "
            "any error types it returns, and how it verifies the token signature."
        )

    @property
    def ground_truth(self) -> GroundTruth:
        return GroundTruth(
            required_strings=["validate_jwt_token", "JwtClaims", "JwtError", "pub fn validate_jwt_token"]
        )

    @property
    def task_type(self) -> str:
        return "read"
