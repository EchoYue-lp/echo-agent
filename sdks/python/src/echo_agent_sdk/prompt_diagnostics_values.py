from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True, slots=True)
class PromptSectionDiagnostic:
    id: str
    source: str


@dataclass(slots=True)
class PromptDiagnostics:
    sections: list[PromptSectionDiagnostic] = field(default_factory=list)

    def record(self, id: str, source: str) -> None:
        self.sections.append(PromptSectionDiagnostic(id=id, source=source))

    def count(self, id: str) -> int:
        return sum(section.id == id for section in self.sections)
