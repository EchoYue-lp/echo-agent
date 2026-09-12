from __future__ import annotations

from dataclasses import dataclass, replace


def _fraction(value: float) -> float:
    if not 0.0 <= value <= 1.0:
        raise ValueError("token budget allocation must be finite and in [0, 1]")
    return value


@dataclass(frozen=True)
class TokenAllocation:
    system_fits: bool
    tool_defs_fit: bool
    conversation_fits: bool
    output_fits: bool
    conversation_excess: int
    usage_pct: float

    def ok(self) -> bool:
        return (
            self.system_fits
            and self.tool_defs_fit
            and self.conversation_fits
            and self.output_fits
        )

    def needs_compression(self) -> bool:
        return self.conversation_excess > 0


@dataclass(frozen=True)
class BudgetReport:
    total_window: int
    system_prompt: int
    system_prompt_budget: int
    tool_definitions: int
    tool_definitions_budget: int
    conversation: int
    conversation_budget: int
    estimated_output: int
    output_budget: int
    usage_pct: float
    needs_compression: bool


@dataclass(frozen=True)
class TokenBudget:
    total_window: int
    system_pct: float = 0.10
    tool_pct: float = 0.05
    output_pct: float = 0.10
    safety_pct: float = 0.10

    @classmethod
    def new(cls, total_window: int) -> TokenBudget:
        if total_window <= 0:
            raise ValueError("token budget total window must be greater than zero")
        return cls(total_window)

    @classmethod
    def default(cls) -> TokenBudget:
        return cls(128_000)

    def with_allocations(
        self, system_pct: float, tool_pct: float, output_pct: float, safety_pct: float
    ) -> TokenBudget:
        values = tuple(
            _fraction(value) for value in (system_pct, tool_pct, output_pct, safety_pct)
        )
        if sum(values) > 1.0:
            raise ValueError("token budget allocations exceed 1.0")
        return replace(
            self,
            system_pct=values[0],
            tool_pct=values[1],
            output_pct=values[2],
            safety_pct=values[3],
        )

    def system_prompt_budget(self) -> int:
        return round(self.total_window * self.system_pct)

    def tool_definitions_budget(self) -> int:
        return round(self.total_window * self.tool_pct)

    def output_budget(self) -> int:
        return round(self.total_window * self.output_pct)

    def safety_budget(self) -> int:
        return round(self.total_window * self.safety_pct)

    def conversation_budget(self) -> int:
        return round(
            self.total_window
            * max(
                0.0,
                1 - self.system_pct - self.tool_pct - self.output_pct - self.safety_pct,
            )
        )

    def allocate(
        self, system_size: int, tool_defs_size: int, conversation_size: int
    ) -> TokenAllocation:
        effective = max(
            0,
            self.total_window
            - self.output_budget()
            - self.safety_budget()
            - system_size
            - tool_defs_size,
        )
        return TokenAllocation(
            system_size <= self.system_prompt_budget(),
            tool_defs_size <= self.tool_definitions_budget(),
            conversation_size <= effective,
            self.output_budget() > 0,
            max(0, conversation_size - effective),
            (system_size + tool_defs_size + conversation_size)
            / self.total_window
            * 100,
        )

    def report(
        self,
        system_size: int,
        tool_defs_size: int,
        conversation_size: int,
        estimated_output: int,
    ) -> BudgetReport:
        allocation = self.allocate(system_size, tool_defs_size, conversation_size)
        return BudgetReport(
            self.total_window,
            system_size,
            self.system_prompt_budget(),
            tool_defs_size,
            self.tool_definitions_budget(),
            conversation_size,
            self.conversation_budget(),
            estimated_output,
            self.output_budget(),
            allocation.usage_pct,
            allocation.needs_compression(),
        )


@dataclass(frozen=True)
class TokenBudgetConfig:
    total_window: int | None = None
    system_pct: float = 0.10
    tool_pct: float = 0.05
    output_pct: float = 0.10
    safety_pct: float = 0.10
    enabled_flag: bool = True

    @classmethod
    def enabled(cls) -> TokenBudgetConfig:
        return cls()

    @classmethod
    def disabled(cls) -> TokenBudgetConfig:
        return cls(enabled_flag=False)

    @property
    def is_enabled(self) -> bool:
        return self.enabled_flag

    def with_total_window(self, window: int) -> TokenBudgetConfig:
        if window <= 0:
            raise ValueError("token budget total window must be greater than zero")
        return replace(self, total_window=window)

    def build(self, fallback_window: int) -> TokenBudget:
        return TokenBudget.new(self.total_window or fallback_window).with_allocations(
            self.system_pct, self.tool_pct, self.output_pct, self.safety_pct
        )


@dataclass(frozen=True)
class LlmTimeouts:
    request: int | None = 60_000
    first_chunk: int | None = 30_000
    idle: int | None = 30_000
    overall: int | None = None

    @classmethod
    def default(cls) -> LlmTimeouts:
        return cls()

    def request_timeout(self) -> int | None:
        return self.request

    def first_chunk_timeout(self) -> int | None:
        return self.first_chunk

    def idle_timeout(self) -> int | None:
        return self.idle

    def overall_timeout(self) -> int | None:
        return self.overall

    def with_request_timeout(self, timeout_ms: int) -> LlmTimeouts:
        return replace(self, request=timeout_ms if timeout_ms > 0 else None)

    def without_request_timeout(self) -> LlmTimeouts:
        return replace(self, request=None)

    def with_first_chunk_timeout(self, timeout_ms: int) -> LlmTimeouts:
        return replace(self, first_chunk=timeout_ms if timeout_ms > 0 else None)

    def without_first_chunk_timeout(self) -> LlmTimeouts:
        return replace(self, first_chunk=None)

    def with_idle_timeout(self, timeout_ms: int) -> LlmTimeouts:
        return replace(self, idle=timeout_ms if timeout_ms > 0 else None)

    def without_idle_timeout(self) -> LlmTimeouts:
        return replace(self, idle=None)

    def with_overall_timeout(self, timeout_ms: int) -> LlmTimeouts:
        return replace(self, overall=timeout_ms if timeout_ms > 0 else None)

    def without_overall_timeout(self) -> LlmTimeouts:
        return replace(self, overall=None)
