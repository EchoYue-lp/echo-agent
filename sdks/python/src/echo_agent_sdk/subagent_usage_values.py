from __future__ import annotations

from dataclasses import dataclass


@dataclass(slots=True)
class LlmUsageStats:
    model: str = ""
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    cached_prompt_tokens: int = 0
    cache_creation_prompt_tokens: int = 0
    usage_reported: bool = False
    call_count: int = 0

    def record(
        self,
        model: str,
        prompt_tokens: int,
        completion_tokens: int,
        total_tokens: int,
        cached_prompt_tokens: int,
        cache_creation_prompt_tokens: int,
        usage_reported: bool,
    ) -> None:
        self.model = model
        self.prompt_tokens += prompt_tokens
        self.completion_tokens += completion_tokens
        self.total_tokens += total_tokens
        self.cached_prompt_tokens += cached_prompt_tokens
        self.cache_creation_prompt_tokens += cache_creation_prompt_tokens
        self.usage_reported = self.usage_reported or usage_reported
        self.call_count += 1

    def to_payload(self, session_id: str) -> dict[str, str | bool | int]:
        return {
            "session_id": session_id,
            "model": self.model or "unknown",
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
            "cached_prompt_tokens": self.cached_prompt_tokens,
            "cache_creation_prompt_tokens": self.cache_creation_prompt_tokens,
            "usage_reported": self.usage_reported,
            "call_count": self.call_count,
        }
