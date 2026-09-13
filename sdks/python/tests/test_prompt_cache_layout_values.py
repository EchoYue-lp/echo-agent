from echo_agent_sdk import PromptCacheLayout, SegmentRanges, to_wire


def _message(role: str, text: str) -> dict[str, object]:
    return {"role": role, "content": to_wire(text)}


def test_prompt_cache_layout_projects_segments_and_ranges() -> None:
    layout = PromptCacheLayout.from_messages(
        [
            _message("system", "You are Echo Agent"),
            _message("system", "[Canonical context - restored]"),
            _message("user", "hello"),
            _message("assistant", "hi"),
            _message("user", "[runtime_context: turn 1]"),
            _message("user", "[runtime_context: hook]"),
        ],
        [{"name": "lookup"}],
    )

    assert len(layout.system) == 1
    assert len(layout.canonical) == 1
    assert len(layout.history) == 2
    assert len(layout.runtime_context) == 2
    assert len(layout.tools) == 1
    ranges = layout.segment_ranges()
    assert ranges == SegmentRanges(
        system=ranges.system,
        canonical=ranges.canonical,
        history=ranges.history,
        runtime_context=ranges.runtime_context,
    )
    assert (ranges.system.start, ranges.system.end) == (0, 1)
    assert (ranges.canonical.start, ranges.canonical.end) == (1, 2)
    assert (ranges.history.start, ranges.history.end) == (2, 4)
    assert (ranges.runtime_context.start, ranges.runtime_context.end) == (4, 6)


def test_prompt_cache_layout_leaves_canonical_empty_without_marker() -> None:
    layout = PromptCacheLayout.from_messages(
        [_message("system", "S"), _message("user", "hello")], []
    )
    assert layout.canonical == ()
    assert len(layout.history) == 1
    assert layout.segment_ranges().canonical.is_empty()
