import pytest

from echo_agent_sdk import (
    IncrementalUtf8Decoder,
    clean_json,
    extract_json_from_markdown,
    split_utf8_chunks,
)


def test_split_utf8_chunks_keeps_scalars_under_the_byte_cap() -> None:
    chunks = split_utf8_chunks("中文ab", 5)

    assert chunks == ["中", "文ab"]
    assert all(len(chunk.encode("utf-8")) <= 5 for chunk in chunks)


def test_split_utf8_chunks_keeps_a_scalar_when_cap_is_smaller_than_it() -> None:
    assert split_utf8_chunks("é", 1) == ["é"]
    assert split_utf8_chunks("", 1) == []


def test_incremental_utf8_decoder_preserves_pending_suffix_and_flushes_it() -> None:
    decoder = IncrementalUtf8Decoder(5)

    assert decoder.push(b"\xe4\xb8") == []
    assert decoder.push(b"\xadab") == ["中ab"]
    assert decoder.finish() is None

    assert decoder.push(b"\xf0\x9f") == []
    assert decoder.finish() == "�"


def test_incremental_utf8_decoder_replaces_invalid_bytes() -> None:
    decoder = IncrementalUtf8Decoder()

    assert decoder.push(b"a\xff") == ["a�"]
    assert decoder.finish() is None


def test_clean_json_removes_only_structural_trailing_commas() -> None:
    value = r'{"text":"keep ,} and ,] literal","escaped":"\\\" ,}",}'

    assert clean_json(value) == r'{"text":"keep ,} and ,] literal","escaped":"\\\" ,}"}'
    assert clean_json('{"中文": 1, \n}') == '{"中文": 1 \n}'
    assert clean_json("{'a': 'don\\'t'}") == "{'a': 'don\\'t'}"


def test_extract_json_from_markdown_supports_json_and_plain_fences() -> None:
    assert (
        extract_json_from_markdown('before\n```json\n{"key": 1}\n```\nafter')
        == '{"key": 1}'
    )
    assert extract_json_from_markdown("```\n[1, 2]\n```") == "[1, 2]"
    assert extract_json_from_markdown('  {"key": 1}  ') == '{"key": 1}'


@pytest.mark.parametrize("value", [None, 1, True])
def test_text_helpers_reject_non_text_values(value: object) -> None:
    with pytest.raises(TypeError):
        clean_json(value)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        extract_json_from_markdown(value)  # type: ignore[arg-type]
    with pytest.raises(TypeError):
        split_utf8_chunks(value, 1)  # type: ignore[arg-type]
