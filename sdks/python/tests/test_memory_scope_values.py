from echo_agent_sdk import (
    MemoryScope,
    memory_scope_all,
    memory_scope_is_persistent,
    memory_scope_name,
    memory_scope_priority,
    parse_memory_scope,
)


def test_memory_scopes_preserve_rust_order_names_and_persistence() -> None:
    assert memory_scope_all() == (
        MemoryScope.USER,
        MemoryScope.PROJECT,
        MemoryScope.REPO,
        MemoryScope.TASK,
        MemoryScope.SESSION,
        MemoryScope.RUN,
    )
    assert memory_scope_name(MemoryScope.PROJECT) == "project"
    assert memory_scope_priority(MemoryScope.RUN) == 5
    assert memory_scope_is_persistent(MemoryScope.USER)
    assert not memory_scope_is_persistent(MemoryScope.TASK)


def test_memory_scope_parser_preserves_aliases_and_unknown_values() -> None:
    assert parse_memory_scope(" proj ") is MemoryScope.PROJECT
    assert parse_memory_scope("SESS") is MemoryScope.SESSION
    assert parse_memory_scope("unknown") is None
    assert parse_memory_scope(42) is None
