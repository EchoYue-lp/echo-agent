from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SkillValidationReport:
    path: str
    violations: tuple[str, ...] = ()
    warnings: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "violations", tuple(self.violations))
        object.__setattr__(self, "warnings", tuple(self.warnings))

    def is_valid(self) -> bool:
        return not self.violations
