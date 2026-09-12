from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class AgentSteerPhase(str, Enum):
    ACCEPTED = "accepted"
    DRAINED = "drained"
    TURN_SETTLED = "turn_settled"


class AgentSteerTurnOutcome(str, Enum):
    COMPLETED = "completed"
    CANCELLED = "cancelled"
    FAILED = "failed"
    DROPPED = "dropped"

    def as_str(self) -> str:
        return self.value

    @classmethod
    def parse(cls, value: str) -> AgentSteerTurnOutcome | None:
        if not isinstance(value, str):
            return None
        try:
            return cls(value)
        except ValueError:
            return None


@dataclass(frozen=True, slots=True)
class AgentSteerState:
    kind: AgentSteerPhase
    outcome: AgentSteerTurnOutcome | None = None
    drained: bool | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.kind, AgentSteerPhase):
            raise TypeError("kind must be an AgentSteerPhase")
        if self.kind is AgentSteerPhase.TURN_SETTLED:
            if not isinstance(self.outcome, AgentSteerTurnOutcome):
                raise TypeError("turn-settled state requires an outcome")
            if not isinstance(self.drained, bool):
                raise TypeError("turn-settled state requires a drained flag")
        elif self.outcome is not None or self.drained is not None:
            raise TypeError("only turn-settled state carries outcome data")

    @classmethod
    def accepted(cls) -> AgentSteerState:
        return cls(AgentSteerPhase.ACCEPTED)

    @classmethod
    def drained_state(cls) -> AgentSteerState:
        return cls(AgentSteerPhase.DRAINED)

    @classmethod
    def turn_settled(
        cls, outcome: AgentSteerTurnOutcome, drained: bool
    ) -> AgentSteerState:
        return cls(AgentSteerPhase.TURN_SETTLED, outcome, drained)

    def phase(self) -> AgentSteerPhase:
        return self.kind

    def was_drained(self) -> bool:
        return self.kind is AgentSteerPhase.DRAINED or (
            self.kind is AgentSteerPhase.TURN_SETTLED and self.drained is True
        )
