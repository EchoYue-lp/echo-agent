from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlparse


@dataclass(frozen=True, slots=True)
class HookAction:
    type: str
    command: str | None = None
    shell: str | None = None
    prompt: str | None = None
    decision: str | None = None
    reason: str | None = None
    suggestions: tuple[str, ...] = ()
    url: str | None = None
    method: str | None = None
    headers: Mapping[str, str] | None = None
    server: str | None = None
    tool: str | None = None
    arguments: Any = None
    name: str | None = None
    task: str | None = None
    skill: str | None = None
    timeout: int = 300

    def kind(self) -> str:
        return self.type

    def validate(self) -> None:
        if self.type == "command":
            if not self.command:
                raise ValueError("Command hook has empty command string")
            if len(self.command) > 32 * 1024:
                raise ValueError("Command hook exceeds max length")
            if self.timeout > 3600:
                raise ValueError("Command hook timeout exceeds maximum")
        elif self.type == "prompt":
            if not self.prompt:
                raise ValueError("Prompt hook has empty prompt string")
        elif self.type == "permission":
            if self.decision not in {"allow", "deny", "ask"}:
                raise ValueError("Permission hook has invalid decision")
        elif self.type == "http":
            parsed = urlparse(self.url or "")
            local = parsed.hostname in {"localhost", "127.0.0.1", "::1"}
            if not self.url:
                raise ValueError("Http hook has empty url")
            if parsed.scheme != "https" and not (parsed.scheme == "http" and local):
                raise ValueError(
                    "Http hook must use https unless it targets a local address"
                )
            if self.timeout > 3600:
                raise ValueError("Http hook timeout exceeds maximum")
        elif self.type == "mcp_tool":
            if not self.server:
                raise ValueError("McpTool hook has empty server name")
            if not self.tool:
                raise ValueError("McpTool hook has empty tool name")
            if self.timeout > 3600:
                raise ValueError("McpTool hook timeout exceeds maximum")
        elif self.type == "subagent":
            if not self.name:
                raise ValueError("Subagent hook has empty subagent name")
            if self.timeout > 3600:
                raise ValueError("Subagent hook timeout exceeds maximum")
        elif self.type == "activate_skill":
            if not self.skill:
                raise ValueError("ActivateSkill hook has empty skill name")
        else:
            raise ValueError(f"unknown hook action type: {self.type}")
