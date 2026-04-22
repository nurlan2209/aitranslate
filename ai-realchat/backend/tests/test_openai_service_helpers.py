from openai_service import OpenAIService


def test_incomplete_utterance_detects_trailing_connector_ru() -> None:
    assert OpenAIService.is_likely_incomplete_utterance("Я хотел уточнить и") is True


def test_incomplete_utterance_detects_short_fragment_en() -> None:
    assert OpenAIService.is_likely_incomplete_utterance("wait") is True


def test_incomplete_utterance_is_false_for_complete_sentence() -> None:
    assert OpenAIService.is_likely_incomplete_utterance("Please explain this step.") is False


def test_build_turn_context_hint_contains_barge_in_details() -> None:
    hint = OpenAIService._build_turn_context_hint(
        "подожди",
        {
            "barge_in": True,
            "interrupted_assistant_text": "Сначала откройте настройки и",
            "interrupted_at_ms": 1100,
            "incomplete_utterance": True,
        },
    )

    assert "interrupted your spoken response" in hint
    assert "Interruption happened around 1100 ms" in hint
    assert "may be incomplete" in hint
