from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SubagentAttemptIdentity:
    task_id: str
    execution_id: str
    attempt: int

    def __post_init__(self) -> None:
        if not self.task_id.strip():
            raise ValueError("invalid identity: task_id")
        if not self.execution_id.strip():
            raise ValueError("invalid identity: execution_id")
        if (
            isinstance(self.attempt, bool)
            or not isinstance(self.attempt, int)
            or not 0 <= self.attempt <= 0xFFFFFFFF
        ):
            raise ValueError("invalid identity: attempt")


@dataclass(frozen=True, slots=True)
class SubagentCommandIdentity:
    run_id: str
    task_id: str
    execution_id: str
    plan_revision: int
    attempt: int
    command_id: str

    def __post_init__(self) -> None:
        self.validate()

    def validate(self) -> None:
        if not self.run_id.strip():
            raise ValueError("invalid identity: run_id")
        if (
            isinstance(self.plan_revision, bool)
            or not isinstance(self.plan_revision, int)
            or self.plan_revision == 0
        ):
            raise ValueError("invalid identity: plan_revision")
        if not self.command_id.strip():
            raise ValueError("invalid identity: command_id")
        self.attempt_identity()

    def attempt_identity(self) -> SubagentAttemptIdentity:
        return SubagentAttemptIdentity(self.task_id, self.execution_id, self.attempt)
