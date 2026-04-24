import json
import os
from dataclasses import dataclass, field
from typing import Any


@dataclass
class ToolCall:
    """Represents a single tool invocation."""
    name: str
    input: dict[str, Any]
    tool_use_id: str
    turn_index: int


@dataclass
class Turn:
    """Represents one assistant turn with usage and tool calls."""
    index: int
    input_tokens: int
    output_tokens: int
    cache_creation_tokens: int
    cache_read_tokens: int
    tool_calls: list[ToolCall] = field(default_factory=list)

    @property
    def context_tokens(self) -> int:
        """Total context processed this turn (input + cached)."""
        return self.input_tokens + self.cache_creation_tokens + self.cache_read_tokens


@dataclass
class RunResult:
    """Complete parsed result from a claude -p run."""
    session_id: str
    turns: list[Turn]
    num_turns: int
    total_cost_usd: float
    duration_ms: int
    duration_api_ms: int
    total_input_tokens: int
    total_output_tokens: int
    total_cache_creation_tokens: int
    total_cache_read_tokens: int
    result_text: str
    task_name: str = ""
    mode_name: str = ""
    model_name: str = ""
    repetition: int = 0
    correct: bool = False
    correctness_reason: str = ""


def parse_stream_json(raw_output: str) -> RunResult:
    """Parse newline-delimited JSON output from claude -p --output-format stream-json --verbose."""
    lines = [line.strip() for line in raw_output.strip().split("\n") if line.strip()]
    events = [json.loads(line) for line in lines]

    session_id = ""
    turns: list[Turn] = []
    result_text = ""
    final_summary = {}
    turn_index = 0

    for event in events:
        event_type = event.get("type")

        if event_type == "system":
            session_id = event.get("session_id", "")

        elif event_type == "assistant":
            message = event.get("message", {})
            usage = message.get("usage", {})
            content_blocks = message.get("content", [])

            tool_calls: list[ToolCall] = []
            text_blocks: list[str] = []

            for block in content_blocks:
                if block.get("type") == "tool_use":
                    tool_calls.append(ToolCall(
                        name=block.get("name", ""),
                        input=block.get("input", {}),
                        tool_use_id=block.get("id", ""),
                        turn_index=turn_index,
                    ))
                elif block.get("type") == "text":
                    text_blocks.append(block.get("text", ""))

            turn = Turn(
                index=turn_index,
                input_tokens=usage.get("input_tokens", 0),
                output_tokens=usage.get("output_tokens", 0),
                cache_creation_tokens=usage.get("cache_creation_input_tokens", 0),
                cache_read_tokens=usage.get("cache_read_input_tokens", 0),
                tool_calls=tool_calls,
            )
            turns.append(turn)
            turn_index += 1

            if text_blocks:
                result_text = "\n".join(text_blocks)

        elif event_type == "result":
            final_summary = event

    return RunResult(
        session_id=session_id,
        turns=turns,
        num_turns=final_summary.get("num_turns", len(turns)),
        total_cost_usd=final_summary.get("total_cost_usd", 0.0),
        duration_ms=final_summary.get("duration_ms", 0),
        duration_api_ms=final_summary.get("duration_api_ms", 0),
        total_input_tokens=final_summary.get("usage", {}).get("input_tokens", 0),
        total_output_tokens=final_summary.get("usage", {}).get("output_tokens", 0),
        total_cache_creation_tokens=final_summary.get("usage", {}).get("cache_creation_input_tokens", 0),
        total_cache_read_tokens=final_summary.get("usage", {}).get("cache_read_input_tokens", 0),
        result_text=result_text,
    )


def tool_call_counts(result: RunResult) -> dict[str, int]:
    """Count tool calls by name across all turns."""
    counts: dict[str, int] = {}
    for turn in result.turns:
        for tool_call in turn.tool_calls:
            counts[tool_call.name] = counts.get(tool_call.name, 0) + 1
    return counts
