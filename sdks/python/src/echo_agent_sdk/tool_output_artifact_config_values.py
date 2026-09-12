from __future__ import annotations

import tempfile
from dataclasses import dataclass, replace
from pathlib import Path


@dataclass(frozen=True, slots=True)
class ToolOutputArtifactConfig:
    root_dir: str
    retention: str
    threshold_bytes: int = 1_048_576
    max_age_secs: int | None = None

    @classmethod
    def new(cls, root_dir: str, retention: str) -> ToolOutputArtifactConfig:
        return cls(root_dir, retention)

    @classmethod
    def default(cls) -> ToolOutputArtifactConfig:
        return cls(
            str(Path(tempfile.gettempdir()) / "echo_agent_artifacts" / "tool-logs"),
            "temporary_1h",
            1_048_576,
            3_600,
        )

    def threshold_bytes_with(self, value: int) -> ToolOutputArtifactConfig:
        return replace(self, threshold_bytes=max(1, value))

    def max_age_secs_with(self, value: int | None) -> ToolOutputArtifactConfig:
        return replace(self, max_age_secs=value)
