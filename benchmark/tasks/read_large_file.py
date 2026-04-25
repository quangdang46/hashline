from .base import Task, GroundTruth


class ReadLargeFileTask(Task):
    @property
    def name(self) -> str:
        return "read_large_file"

    @property
    def prompt(self) -> str:
        return (
            "Find the rate limiting logic in this codebase. "
            "Show the RateLimiter struct and how it implements the sliding window algorithm. "
            "What methods does it provide?"
        )

    @property
    def ground_truth(self) -> GroundTruth:
        return GroundTruth(
            required_strings=["RateLimiter", "requests", "is_allowed", "window_secs"]
        )

    @property
    def task_type(self) -> str:
        return "read"