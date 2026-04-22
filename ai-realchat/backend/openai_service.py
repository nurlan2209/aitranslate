import io
import os
import re
from typing import Any, Dict, List

from openai import OpenAI

SYSTEM_PROMPT = """Ты мультиязычный AI-ассистент для голоса и чата.

Правила:
1. Автоматически определяй язык сообщения пользователя.
2. Всегда отвечай на том же языке, что и последнее сообщение пользователя.
3. Если пользователь переключил язык, сразу переключай язык ответа.
4. Пиши коротко и естественно, как для живого диалога.
5. Будь вежливым и разговорным.
6. Если запрос неясен, задай один короткий уточняющий вопрос.
7. Если не уверен, прямо скажи, что не уверен, и не выдумывай факты.

Поддерживаемые языки: русский, казахский, английский.
"""


class OpenAIService:
    def __init__(self) -> None:
        api_key = os.getenv("OPENAI_API_KEY")
        if not api_key:
            raise RuntimeError("OPENAI_API_KEY is not set")

        self.client = OpenAI(api_key=api_key)
        self.stt_model = os.getenv("OPENAI_STT_MODEL", "gpt-4o-mini-transcribe")
        self.chat_model = os.getenv("OPENAI_CHAT_MODEL", "gpt-4o-mini")
        self.tts_model = os.getenv("OPENAI_TTS_MODEL", "gpt-4o-mini-tts")
        self.tts_voice = os.getenv("OPENAI_TTS_VOICE", "alloy")
        self.tts_format = os.getenv("OPENAI_TTS_FORMAT", "mp3")

    def transcribe_audio(self, audio_bytes: bytes, filename: str) -> str:
        audio_file = io.BytesIO(audio_bytes)
        audio_file.name = filename

        transcription = self.client.audio.transcriptions.create(
            model=self.stt_model,
            file=audio_file,
        )
        text = (getattr(transcription, "text", "") or "").strip()
        if not text:
            raise RuntimeError("Could not transcribe audio")
        return text

    def generate_reply(
        self,
        user_text: str,
        history: List[Dict[str, str]],
        turn_meta: Dict[str, Any] | None = None,
    ) -> str:
        messages: List[Dict[str, str]] = [{"role": "system", "content": SYSTEM_PROMPT}]
        turn_hint = self._build_turn_context_hint(user_text, turn_meta or {})
        if turn_hint:
            messages.append({"role": "system", "content": turn_hint})
        messages.extend(self._normalize_history(history))
        messages.append({"role": "user", "content": user_text})

        completion = self.client.chat.completions.create(
            model=self.chat_model,
            messages=messages,
            temperature=0.6,
            max_tokens=180,
        )

        content = completion.choices[0].message.content
        if isinstance(content, list):
            # Defensive fallback for SDK variants with structured content blocks.
            text_parts = [str(block.get("text", "")) for block in content if isinstance(block, dict)]
            reply = " ".join(p for p in text_parts if p).strip()
        else:
            reply = (content or "").strip()

        if not reply:
            raise RuntimeError("Model returned an empty response")
        return reply

    @staticmethod
    def _build_turn_context_hint(user_text: str, turn_meta: Dict[str, Any]) -> str:
        hints: List[str] = []
        barge_in = bool(turn_meta.get("barge_in", False))
        incomplete = bool(turn_meta.get("incomplete_utterance", False))
        interrupted_text = str(turn_meta.get("interrupted_assistant_text", "")).strip()
        interrupted_at_ms = turn_meta.get("interrupted_at_ms")

        if barge_in:
            hints.append(
                "Conversation event: user interrupted your spoken response and started talking."
            )
            if interrupted_text:
                excerpt = " ".join(interrupted_text.split())[:160]
                hints.append(f'Interrupted response excerpt: "{excerpt}"')
            if isinstance(interrupted_at_ms, int):
                hints.append(f"Interruption happened around {interrupted_at_ms} ms.")
            hints.append(
                "Treat the latest user message as priority correction or continuation, and avoid repeating your full previous answer."
            )

        if incomplete or OpenAIService.is_likely_incomplete_utterance(user_text):
            hints.append(
                "Latest user utterance may be incomplete. If intent is unclear, ask one short clarifying question in the same language."
            )

        return " ".join(hints).strip()

    @staticmethod
    def is_likely_incomplete_utterance(text: str) -> bool:
        normalized = " ".join((text or "").strip().split())
        if not normalized:
            return False

        if normalized.endswith(("...", "…", ",", ";", ":", "-", "—")):
            return True

        terminal_punctuation = (".", "!", "?")
        lower = normalized.lower()
        tokens = [t for t in re.split(r"\s+", lower) if t]
        if not tokens:
            return False

        if len(tokens) <= 2 and not normalized.endswith(terminal_punctuation):
            return True

        connector_words = {
            "ru": {"и", "или", "но", "а", "если", "когда", "чтобы"},
            "kk": {"және", "немесе", "бірақ", "ал", "егер", "қашан", "үшін"},
            "en": {"and", "or", "but", "if", "when", "because", "so"},
        }
        all_connectors = set().union(*connector_words.values())
        last_token = re.sub(r"[^\w\u0400-\u04FF\u0490-\u04FF]+$", "", tokens[-1])
        if last_token in all_connectors and not normalized.endswith(terminal_punctuation):
            return True

        trailing_phrases = [
            "потому что",
            "так как",
            "из-за того что",
            "өйткені",
            "себебі",
            "because",
            "so that",
        ]
        for phrase in trailing_phrases:
            if lower.endswith(phrase):
                return True

        return False

    def synthesize_speech(self, text: str) -> tuple[bytes, str]:
        speech = self.client.audio.speech.create(
            model=self.tts_model,
            voice=self.tts_voice,
            input=text,
            response_format=self.tts_format,
        )

        audio_bytes = self._extract_binary(speech)
        if not audio_bytes:
            raise RuntimeError("Could not synthesize speech")

        return audio_bytes, self._mime_for_format(self.tts_format)

    @staticmethod
    def _normalize_history(history: List[Dict[str, str]]) -> List[Dict[str, str]]:
        normalized: List[Dict[str, str]] = []
        for item in history:
            if not isinstance(item, dict):
                continue
            role = str(item.get("role", "")).strip()
            content = str(item.get("content", "")).strip()
            if role not in {"user", "assistant"}:
                continue
            if not content:
                continue
            normalized.append({"role": role, "content": content})
        return normalized[-12:]

    @staticmethod
    def _extract_binary(response_obj: Any) -> bytes:
        if hasattr(response_obj, "read"):
            data = response_obj.read()
            if isinstance(data, (bytes, bytearray)):
                return bytes(data)

        content = getattr(response_obj, "content", None)
        if isinstance(content, (bytes, bytearray)):
            return bytes(content)

        if isinstance(response_obj, (bytes, bytearray)):
            return bytes(response_obj)

        return b""

    @staticmethod
    def _mime_for_format(fmt: str) -> str:
        f = fmt.lower()
        if f == "mp3":
            return "audio/mpeg"
        if f == "wav":
            return "audio/wav"
        if f == "aac":
            return "audio/aac"
        if f == "opus":
            return "audio/opus"
        if f == "flac":
            return "audio/flac"
        return "application/octet-stream"
