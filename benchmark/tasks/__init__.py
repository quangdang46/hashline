"""Tasks package for linehash benchmarks."""

from .line_navigation import LineNavigationTask
from .edit_verification import EditVerificationTask


TASKS = {
    "line_navigation": LineNavigationTask(),
    "edit_verification": EditVerificationTask(),
}
