from .base import Task, GroundTruth


class MarkdownSectionTask(Task):
    @property
    def name(self) -> str:
        return "markdown_section"

    @property
    def prompt(self) -> str:
        return (
            "Read the Deployment section from README.md. "
            "What environment variables are required and what does each do?"
        )

    @property
    def ground_truth(self) -> GroundTruth:
        return GroundTruth(
            required_strings=["DATABASE_URL", "SECRET_KEY", "REDIS_URL", "PORT", "LOG_LEVEL"]
        )

    @property
    def task_type(self) -> str:
        return "read"