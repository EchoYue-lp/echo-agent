from __future__ import annotations

from enum import Enum


class TaskState(str, Enum):
    """Closed A2A task state with the Rust transition rules."""

    SUBMITTED = "submitted"
    WORKING = "working"
    INPUT_REQUIRED = "input-required"
    COMPLETED = "completed"
    FAILED = "failed"
    CANCELED = "canceled"

    def is_terminal(self) -> bool:
        return self in {self.COMPLETED, self.FAILED, self.CANCELED}

    def can_transition_to(self, next_state: TaskState) -> bool:
        if not isinstance(next_state, TaskState):
            raise TypeError(f"unknown A2A task state: {next_state}")
        if self.is_terminal():
            return False
        return (self, next_state) in {
            (self.SUBMITTED, self.WORKING),
            (self.SUBMITTED, self.CANCELED),
            (self.WORKING, self.COMPLETED),
            (self.WORKING, self.FAILED),
            (self.WORKING, self.INPUT_REQUIRED),
            (self.WORKING, self.CANCELED),
            (self.INPUT_REQUIRED, self.WORKING),
            (self.INPUT_REQUIRED, self.CANCELED),
        }

    def __str__(self) -> str:
        return self.value
