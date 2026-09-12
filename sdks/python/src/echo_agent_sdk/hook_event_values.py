from __future__ import annotations

from enum import Enum
from typing import Final


class HookEventCategory(str, Enum):
    TOOL = "Tool"
    LIFECYCLE = "Lifecycle"
    SUBAGENT = "Subagent"
    TASK = "Task"
    ERROR = "Error"
    EVOLUTION = "Evolution"


class HookEvent(str, Enum):
    PRE_TOOL_USE = "PreToolUse"
    POST_TOOL_USE = "PostToolUse"
    POST_TOOL_USE_FAILURE = "PostToolUseFailure"
    PERMISSION_REQUEST = "PermissionRequest"
    PERMISSION_DENIED = "PermissionDenied"
    SESSION_START = "SessionStart"
    SESSION_END = "SessionEnd"
    STOP = "Stop"
    NOTIFICATION = "Notification"
    USER_PROMPT_SUBMIT = "UserPromptSubmit"
    PRE_COMPACT = "PreCompact"
    POST_COMPACT = "PostCompact"
    CONFIG_CHANGE = "ConfigChange"
    INSTRUCTIONS_LOADED = "InstructionsLoaded"
    POST_TOOL_BATCH = "PostToolBatch"
    SUBAGENT_START = "SubagentStart"
    SUBAGENT_STOP = "SubagentStop"
    TASK_CREATED = "TaskCreated"
    TASK_STARTED = "TaskStarted"
    TASK_COMPLETED = "TaskCompleted"
    STOP_FAILURE = "StopFailure"
    PLUGIN_LOADED = "PluginLoaded"
    PLUGIN_DISABLED = "PluginDisabled"
    POST_MEMORY_WRITE = "PostMemoryWrite"
    MEMORY_LAYER_CHANGE = "MemoryLayerChange"
    SKILL_CANDIDATE_DETECTED = "SkillCandidateDetected"
    SKILL_LIFECYCLE_TRANSITION = "SkillLifecycleTransition"
    SKILL_HEALTH_CHECK = "SkillHealthCheck"
    SKILL_PATCH_APPLIED = "SkillPatchApplied"
    SKILL_MERGE_APPLIED = "SkillMergeApplied"
    RULE_PROMOTED = "RulePromoted"

    def as_str(self) -> str:
        return self.value

    @classmethod
    def from_name(cls, name: str) -> HookEvent | None:
        try:
            return cls(name)
        except ValueError:
            return None

    def category(self) -> HookEventCategory:
        if self in _TOOL_EVENTS:
            return HookEventCategory.TOOL
        if self in _SUBAGENT_EVENTS:
            return HookEventCategory.SUBAGENT
        if self in _TASK_EVENTS:
            return HookEventCategory.TASK
        if self is HookEvent.STOP_FAILURE:
            return HookEventCategory.ERROR
        if self in _EVOLUTION_EVENTS:
            return HookEventCategory.EVOLUTION
        return HookEventCategory.LIFECYCLE

    def is_tool_event(self) -> bool:
        return self in _TOOL_EVENTS

    def supports_matcher(self) -> bool:
        return True


HOOK_EVENT_ALL: Final[tuple[HookEvent, ...]] = tuple(HookEvent)
_TOOL_EVENTS = frozenset(
    {
        HookEvent.PRE_TOOL_USE,
        HookEvent.POST_TOOL_USE,
        HookEvent.POST_TOOL_USE_FAILURE,
        HookEvent.PERMISSION_REQUEST,
        HookEvent.PERMISSION_DENIED,
    }
)
_SUBAGENT_EVENTS = frozenset({HookEvent.SUBAGENT_START, HookEvent.SUBAGENT_STOP})
_TASK_EVENTS = frozenset(
    {HookEvent.TASK_CREATED, HookEvent.TASK_STARTED, HookEvent.TASK_COMPLETED}
)
_EVOLUTION_EVENTS = frozenset(
    {
        HookEvent.POST_MEMORY_WRITE,
        HookEvent.MEMORY_LAYER_CHANGE,
        HookEvent.SKILL_CANDIDATE_DETECTED,
        HookEvent.SKILL_LIFECYCLE_TRANSITION,
        HookEvent.SKILL_HEALTH_CHECK,
        HookEvent.SKILL_PATCH_APPLIED,
        HookEvent.SKILL_MERGE_APPLIED,
        HookEvent.RULE_PROMOTED,
    }
)
