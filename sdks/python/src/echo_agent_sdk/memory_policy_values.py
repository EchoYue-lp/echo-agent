from __future__ import annotations

from enum import Enum


class MemoryType(str, Enum):
    USER_PREFERENCE = "user_preference"
    PROJECT_FACT = "project_fact"
    ARCHITECTURE_DECISION = "architecture_decision"
    DEBUGGING_LESSON = "debugging_lesson"
    ERROR_RESOLUTION = "error_resolution"
    COMMAND_PATTERN = "command_pattern"
    TOOL_USAGE = "tool_usage"
    WORKFLOW_PATTERN = "workflow_pattern"
    SKILL_CANDIDATE = "skill_candidate"
    DEPRECATED_NOTE = "deprecated_note"


class MemorySource(str, Enum):
    USER_CORRECTION = "user_correction"
    ERROR_RESOLUTION = "error_resolution"
    REPEATED_WORKFLOW = "repeated_workflow"
    EXPLICIT_SAVE = "explicit_save"
    AUTO_EXTRACTED = "auto_extracted"
    L3_PROMOTION = "l3_promotion"


_TYPE_STABILITY = {
    MemoryType.USER_PREFERENCE: 0.85,
    MemoryType.PROJECT_FACT: 0.60,
    MemoryType.ARCHITECTURE_DECISION: 0.80,
    MemoryType.DEBUGGING_LESSON: 0.55,
    MemoryType.ERROR_RESOLUTION: 0.50,
    MemoryType.COMMAND_PATTERN: 0.40,
    MemoryType.TOOL_USAGE: 0.50,
    MemoryType.WORKFLOW_PATTERN: 0.60,
    MemoryType.SKILL_CANDIDATE: 0.55,
    MemoryType.DEPRECATED_NOTE: 0.10,
}
_SOURCE_CONFIDENCE = {
    MemorySource.USER_CORRECTION: 0.90,
    MemorySource.ERROR_RESOLUTION: 0.85,
    MemorySource.REPEATED_WORKFLOW: 0.75,
    MemorySource.EXPLICIT_SAVE: 1.0,
    MemorySource.AUTO_EXTRACTED: 0.60,
    MemorySource.L3_PROMOTION: 0.50,
}
_SOURCE_RECALL_WEIGHT = {
    MemorySource.USER_CORRECTION: 0.90,
    MemorySource.ERROR_RESOLUTION: 0.70,
    MemorySource.REPEATED_WORKFLOW: 0.60,
    MemorySource.EXPLICIT_SAVE: 0.80,
    MemorySource.AUTO_EXTRACTED: 0.40,
    MemorySource.L3_PROMOTION: 0.40,
}


def memory_type_default_stability(memory_type: MemoryType) -> float:
    return _TYPE_STABILITY[_validate_type(memory_type)]


def memory_type_is_skill_eligible(memory_type: MemoryType) -> bool:
    return _validate_type(memory_type) in {
        MemoryType.WORKFLOW_PATTERN,
        MemoryType.SKILL_CANDIDATE,
        MemoryType.DEBUGGING_LESSON,
    }


def memory_type_is_rule_eligible(memory_type: MemoryType) -> bool:
    return _validate_type(memory_type) in {
        MemoryType.USER_PREFERENCE,
        MemoryType.ARCHITECTURE_DECISION,
        MemoryType.PROJECT_FACT,
    }


def memory_source_default_confidence(source: MemorySource) -> float:
    return _SOURCE_CONFIDENCE[_validate_source(source)]


def memory_source_default_recall_weight(source: MemorySource) -> float:
    return _SOURCE_RECALL_WEIGHT[_validate_source(source)]


def _validate_type(value: MemoryType) -> MemoryType:
    if not isinstance(value, MemoryType):
        raise TypeError(f"unknown memory type: {value!r}")
    return value


def _validate_source(value: MemorySource) -> MemorySource:
    if not isinstance(value, MemorySource):
        raise TypeError(f"unknown memory source: {value!r}")
    return value
