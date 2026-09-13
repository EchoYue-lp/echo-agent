from echo_agent_sdk import (
    MemorySource,
    MemoryType,
    memory_source_default_confidence,
    memory_source_default_recall_weight,
    memory_type_default_stability,
    memory_type_is_rule_eligible,
    memory_type_is_skill_eligible,
)


def test_memory_type_policies_preserve_rust_stability_and_eligibility() -> None:
    assert memory_type_default_stability(MemoryType.USER_PREFERENCE) == 0.85
    assert memory_type_default_stability(MemoryType.DEPRECATED_NOTE) == 0.1
    assert memory_type_is_rule_eligible(MemoryType.PROJECT_FACT)
    assert not memory_type_is_rule_eligible(MemoryType.WORKFLOW_PATTERN)
    assert memory_type_is_skill_eligible(MemoryType.DEBUGGING_LESSON)
    assert not memory_type_is_skill_eligible(MemoryType.USER_PREFERENCE)


def test_memory_source_policies_preserve_confidence_and_recall_weight() -> None:
    assert memory_source_default_confidence(MemorySource.EXPLICIT_SAVE) == 1.0
    assert memory_source_default_confidence(MemorySource.AUTO_EXTRACTED) == 0.6
    assert memory_source_default_recall_weight(MemorySource.USER_CORRECTION) == 0.9
    assert memory_source_default_recall_weight(MemorySource.L3_PROMOTION) == 0.4
