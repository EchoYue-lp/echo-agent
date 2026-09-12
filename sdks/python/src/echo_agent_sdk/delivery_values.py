from __future__ import annotations

from enum import Enum


class DeliveryOutcome(str, Enum):
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELLED = "cancelled"
    DROPPED = "dropped"
    OUTCOME_UNKNOWN = "outcome_unknown"

    def as_str(self) -> str:
        return self.value


class DeliveryPhase(str, Enum):
    PERSISTED = "persisted"
    CLAIMED = "claimed"
    EFFECT_STARTED = "effect_started"
    MAILBOX_ACCEPTED = "mailbox_accepted"
    DRAINED = "drained"
    DEFERRED = "deferred"
    TURN_SETTLED = "turn_settled"

    def as_str(self) -> str:
        return self.value
