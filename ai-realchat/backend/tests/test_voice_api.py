import base64
import json

from fastapi.testclient import TestClient

import main


class FakeOpenAIService:
    def __init__(self) -> None:
        self.received_user_text = ""
        self.received_history = []
        self.received_turn_meta = {}
        self.synth_calls = 0

    def transcribe_audio(self, audio_bytes: bytes, filename: str) -> str:
        assert audio_bytes
        assert filename
        return "Please continue."

    def generate_reply(self, user_text, history, turn_meta=None):
        self.received_user_text = user_text
        self.received_history = history
        self.received_turn_meta = turn_meta or {}
        return "Okay, let's continue."

    def synthesize_speech(self, text: str):
        assert text
        self.synth_calls += 1
        return b"voice-bytes", "audio/mpeg"


def test_voice_endpoint_handles_barge_in_metadata(monkeypatch) -> None:
    fake = FakeOpenAIService()
    monkeypatch.setattr(main, "get_service", lambda: fake)

    client = TestClient(main.app)
    response = client.post(
        "/api/voice",
        files={"file": ("speech.webm", b"abc", "audio/webm")},
        data={
            "history": json.dumps([
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"},
            ]),
            "turn_meta": json.dumps(
                {
                    "barge_in": True,
                    "interrupted_assistant_text": "Hi there, first do this",
                    "interrupted_at_ms": 900,
                }
            ),
        },
    )

    assert response.status_code == 200
    body = response.json()
    assert body["transcript"] == "Please continue."
    assert body["response_text"] == "Okay, let's continue."
    assert body["audio_mime_type"] == "audio/mpeg"
    assert base64.b64decode(body["audio_base64"]) == b"voice-bytes"

    assert fake.received_user_text == "Please continue."
    assert fake.received_turn_meta["barge_in"] is True
    assert fake.received_turn_meta["interrupted_assistant_text"] == "Hi there, first do this"
    assert fake.received_turn_meta["interrupted_at_ms"] == 900
    assert fake.received_turn_meta["incomplete_utterance"] is False


def test_chat_endpoint_returns_text_only_by_default(monkeypatch) -> None:
    fake = FakeOpenAIService()
    monkeypatch.setattr(main, "get_service", lambda: fake)

    client = TestClient(main.app)
    response = client.post(
        "/api/chat",
        json={
            "message": "Hello there",
            "history": [{"role": "assistant", "content": "Hi"}],
        },
    )

    assert response.status_code == 200
    body = response.json()
    assert body["response_text"] == "Okay, let's continue."
    assert "audio_base64" not in body
    assert fake.synth_calls == 0
    assert fake.received_user_text == "Hello there"


def test_chat_endpoint_can_return_audio(monkeypatch) -> None:
    fake = FakeOpenAIService()
    monkeypatch.setattr(main, "get_service", lambda: fake)

    client = TestClient(main.app)
    response = client.post(
        "/api/chat",
        json={
            "message": "Wait and",
            "history": [],
            "speak": True,
        },
    )

    assert response.status_code == 200
    body = response.json()
    assert body["response_text"] == "Okay, let's continue."
    assert base64.b64decode(body["audio_base64"]) == b"voice-bytes"
    assert body["audio_mime_type"] == "audio/mpeg"
    assert fake.synth_calls == 1
    assert fake.received_turn_meta["incomplete_utterance"] is True
