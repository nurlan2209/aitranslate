import json

from main import _parse_history_string, _parse_turn_meta


def test_parse_history_keeps_only_valid_roles_and_limits_to_12() -> None:
    payload = [
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "hi"},
        {"role": "system", "content": "skip"},
        {"role": "assistant", "content": " "},
        "bad",
    ] + [{"role": "user", "content": f"m{i}"} for i in range(20)]

    parsed = _parse_history_string(json.dumps(payload))

    assert len(parsed) == 12
    assert all(item["role"] in {"user", "assistant"} for item in parsed)
    assert parsed[0]["content"] == "m8"
    assert parsed[-1]["content"] == "m19"


def test_parse_turn_meta_defaults_for_invalid_json() -> None:
    parsed = _parse_turn_meta("not json")

    assert parsed == {
        "barge_in": False,
        "interrupted_assistant_text": "",
        "interrupted_at_ms": None,
    }


def test_parse_turn_meta_sanitizes_values() -> None:
    huge_text = "a" * 600
    parsed = _parse_turn_meta(
        '{"barge_in": 1, "interrupted_assistant_text": "%s", "interrupted_at_ms": 9999999}'
        % huge_text
    )

    assert parsed["barge_in"] is True
    assert len(parsed["interrupted_assistant_text"]) == 400
    assert parsed["interrupted_at_ms"] == 600000
