from __future__ import annotations

from dataclasses import dataclass

MAX_U32 = (1 << 32) - 1


@dataclass(frozen=True, slots=True)
class TokenUsageDetails:
    cached_tokens: int | None = None
    cache_write_tokens: int | None = None
    reasoning_tokens: int | None = None

    def __post_init__(self) -> None:
        _validate_u32_values(
            self.cached_tokens, self.cache_write_tokens, self.reasoning_tokens
        )


@dataclass(frozen=True, slots=True)
class Usage:
    prompt_tokens: int | None = None
    completion_tokens: int | None = None
    total_tokens: int | None = None
    prompt_tokens_details: TokenUsageDetails | None = None
    input_tokens_details: TokenUsageDetails | None = None
    output_tokens_details: TokenUsageDetails | None = None
    cache_creation_input_tokens: int | None = None
    cache_read_input_tokens: int | None = None
    prompt_cache_hit_tokens: int | None = None
    prompt_cache_miss_tokens: int | None = None

    def __post_init__(self) -> None:
        _validate_u32_values(
            self.prompt_tokens,
            self.completion_tokens,
            self.total_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
            self.prompt_cache_hit_tokens,
            self.prompt_cache_miss_tokens,
        )

    def cached_prompt_tokens(self) -> int:
        return next(
            (
                value
                for value in (
                    self.prompt_tokens_details.cached_tokens
                    if self.prompt_tokens_details
                    else None,
                    self.input_tokens_details.cached_tokens
                    if self.input_tokens_details
                    else None,
                    self.cache_read_input_tokens,
                    self.prompt_cache_hit_tokens,
                )
                if value is not None
            ),
            0,
        )

    def cache_creation_prompt_tokens(self) -> int:
        return next(
            (
                value
                for value in (
                    self.prompt_tokens_details.cache_write_tokens
                    if self.prompt_tokens_details
                    else None,
                    self.input_tokens_details.cache_write_tokens
                    if self.input_tokens_details
                    else None,
                    self.cache_creation_input_tokens,
                )
                if value is not None
            ),
            0,
        )

    def effective_prompt_tokens(self) -> int:
        prompt = self.prompt_tokens or 0
        if (
            self.cache_read_input_tokens is not None
            or self.cache_creation_input_tokens is not None
        ):
            return min(
                MAX_U32,
                prompt
                + self.cached_prompt_tokens()
                + self.cache_creation_prompt_tokens(),
            )
        return prompt

    def effective_total_tokens(self) -> int:
        completion = self.completion_tokens or 0
        if (
            self.cache_read_input_tokens is not None
            or self.cache_creation_input_tokens is not None
        ):
            return min(MAX_U32, self.effective_prompt_tokens() + completion)
        return (
            self.total_tokens
            if self.total_tokens is not None
            else min(MAX_U32, self.effective_prompt_tokens() + completion)
        )

    def cache_hit_rate(self) -> float | None:
        total = self.effective_prompt_tokens()
        return None if total == 0 else self.cached_prompt_tokens() / total


def _validate_u32_values(*values: int | None) -> None:
    if any(
        value is not None
        and (
            isinstance(value, bool)
            or not isinstance(value, int)
            or not 0 <= value <= MAX_U32
        )
        for value in values
    ):
        raise ValueError("usage token values must fit u32")
