from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class ToolPermission(str, Enum):
    READ = "read"
    WRITE = "write"
    NETWORK = "network"
    EXECUTE = "execute"
    SENSITIVE = "sensitive"

    def __str__(self) -> str:
        return self.value


class RuleMatcherKind(str, Enum):
    TOOL = "tool"
    PATTERN = "pattern"
    PERMISSION = "permission"
    ALL = "all"


@dataclass(frozen=True, slots=True)
class RuleMatcher:
    kind: RuleMatcherKind
    value: str | ToolPermission | None = None

    @classmethod
    def parse(cls, value: str) -> RuleMatcher:
        if not isinstance(value, str):
            raise TypeError("permission matcher must be text")
        if value == "*" or value == "all":
            return cls(RuleMatcherKind.ALL)
        if value.startswith("tool:"):
            name = value[5:]
            if not name:
                raise ValueError("tool permission matcher requires a name")
            return cls(RuleMatcherKind.TOOL, name)
        if value.startswith("pattern:"):
            pattern = value[8:]
            if not pattern:
                raise ValueError("pattern permission matcher cannot be empty")
            return cls(RuleMatcherKind.PATTERN, pattern)
        flag = (
            value[5:]
            if value.startswith("perm:")
            else value[11:]
            if value.startswith("permission:")
            else None
        )
        if flag is not None:
            try:
                return cls(RuleMatcherKind.PERMISSION, ToolPermission(flag))
            except ValueError as error:
                raise ValueError(f"unknown permission matcher: {flag}") from error
        raise ValueError(f"unsupported permission matcher: {value}")

    def display(self) -> str:
        if self.kind is RuleMatcherKind.TOOL:
            return f"tool:{self.value}"
        if self.kind is RuleMatcherKind.PATTERN:
            return f"pattern:{self.value}"
        if self.kind is RuleMatcherKind.PERMISSION:
            return f"permission:{self.value}"
        return "all"

    def matches_matcher_str(self, value: str) -> bool:
        return (
            self.kind in {RuleMatcherKind.TOOL, RuleMatcherKind.PATTERN}
            and self.value == value
        ) or (self.kind is RuleMatcherKind.ALL and value in {"*", "all"})

    def matches(
        self,
        tool_name: str,
        permissions: tuple[ToolPermission, ...] | list[ToolPermission],
    ) -> bool:
        if self.kind is RuleMatcherKind.TOOL:
            return tool_name == self.value
        if self.kind is RuleMatcherKind.PERMISSION:
            return self.value in permissions
        if self.kind is RuleMatcherKind.ALL:
            return True
        pattern = str(self.value)
        if pattern == "*":
            return True
        if tool_name == pattern:
            return True
        if _glob_matches(pattern, tool_name):
            return True
        if pattern.endswith("*)") and tool_name.startswith(pattern[:-2]):
            return True
        return (
            tool_name.startswith(pattern)
            and len(tool_name) > len(pattern)
            and tool_name[len(pattern)] == "("
        )


def _glob_matches(pattern: str, value: str) -> bool:
    pattern_index = 0
    value_index = 0
    star = -1
    mark = -1
    while value_index < len(value):
        if pattern_index < len(pattern) and (
            pattern[pattern_index] == "?"
            or pattern[pattern_index] == value[value_index]
        ):
            pattern_index += 1
            value_index += 1
        elif pattern_index < len(pattern) and pattern[pattern_index] == "*":
            star = pattern_index
            pattern_index += 1
            mark = value_index
        elif star != -1:
            pattern_index = star + 1
            mark += 1
            value_index = mark
        else:
            return False
    while pattern_index < len(pattern) and pattern[pattern_index] == "*":
        pattern_index += 1
    return pattern_index == len(pattern)
