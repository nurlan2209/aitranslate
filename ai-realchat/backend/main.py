import base64
import json
import os
from typing import Any, Dict, List

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

from openai_service import OpenAIService

app = FastAPI(title="AI RealChat Voice API")

frontend_origin = os.getenv("FRONTEND_ORIGIN", "http://localhost:3040")
app.add_middleware(
    CORSMiddleware,
    allow_origins=[frontend_origin],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

_service: OpenAIService | None = None


def get_service() -> OpenAIService:
    global _service
    if _service is None:
        _service = OpenAIService()
    return _service


class ChatRequest(BaseModel):
    message: str = Field(default="", max_length=4000)
    history: List[Dict[str, Any]] = Field(default_factory=list)
    turn_meta: Dict[str, Any] = Field(default_factory=dict)
    speak: bool = False


@app.get("/health")
def health() -> Dict[str, str]:
    return {"status": "ok"}


@app.post("/api/voice")
async def process_voice(
    file: UploadFile = File(...),
    history: str = Form("[]"),
    turn_meta: str = Form("{}"),
) -> JSONResponse:
    try:
        audio_bytes = await file.read()
        if not audio_bytes:
            raise HTTPException(status_code=400, detail="Empty audio file")

        service = get_service()
        parsed_history = _parse_history_string(history)
        parsed_turn_meta = _parse_turn_meta(turn_meta)

        transcript = service.transcribe_audio(audio_bytes, file.filename or "speech.webm")
        parsed_turn_meta["incomplete_utterance"] = OpenAIService.is_likely_incomplete_utterance(
            transcript
        )
        reply_text = service.generate_reply(transcript, parsed_history, parsed_turn_meta)
        tts_audio, mime_type = service.synthesize_speech(reply_text)

        payload = {
            "transcript": transcript,
            "response_text": reply_text,
            "audio_base64": base64.b64encode(tts_audio).decode("utf-8"),
            "audio_mime_type": mime_type,
        }
        return JSONResponse(payload)
    except HTTPException:
        raise
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


@app.post("/api/chat")
def process_chat(req: ChatRequest) -> JSONResponse:
    try:
        service = get_service()
        user_text = req.message.strip()
        if not user_text:
            raise HTTPException(status_code=400, detail="Empty message")

        parsed_history = _normalize_history(req.history)
        parsed_turn_meta = _parse_turn_meta_object(req.turn_meta)
        parsed_turn_meta["incomplete_utterance"] = OpenAIService.is_likely_incomplete_utterance(
            user_text
        )

        reply_text = service.generate_reply(user_text, parsed_history, parsed_turn_meta)

        payload: Dict[str, Any] = {
            "response_text": reply_text,
        }
        if req.speak:
            tts_audio, mime_type = service.synthesize_speech(reply_text)
            payload["audio_base64"] = base64.b64encode(tts_audio).decode("utf-8")
            payload["audio_mime_type"] = mime_type

        return JSONResponse(payload)
    except HTTPException:
        raise
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


@app.get("/")
def root() -> Dict[str, str]:
    return {"message": "AI RealChat backend is running"}


def _parse_history_string(history: str) -> List[Dict[str, str]]:
    try:
        parsed = json.loads(history)
    except json.JSONDecodeError:
        return []
    return _normalize_history(parsed)


def _normalize_history(parsed: Any) -> List[Dict[str, str]]:
    if not isinstance(parsed, list):
        return []

    clean: List[Dict[str, str]] = []
    for item in parsed:
        if not isinstance(item, dict):
            continue
        role = str(item.get("role", "")).strip()
        content = str(item.get("content", "")).strip()
        if role not in {"user", "assistant"}:
            continue
        if not content:
            continue
        clean.append({"role": role, "content": content})

    return clean[-12:]


def _parse_turn_meta(turn_meta: str) -> Dict[str, Any]:
    default = {
        "barge_in": False,
        "interrupted_assistant_text": "",
        "interrupted_at_ms": None,
    }

    try:
        parsed = json.loads(turn_meta)
    except json.JSONDecodeError:
        return default

    if not isinstance(parsed, dict):
        return default
    return _parse_turn_meta_object(parsed)


def _parse_turn_meta_object(parsed: Any) -> Dict[str, Any]:
    default = {
        "barge_in": False,
        "interrupted_assistant_text": "",
        "interrupted_at_ms": None,
    }

    if not isinstance(parsed, dict):
        return default

    barge_in = bool(parsed.get("barge_in", False))
    interrupted_text = str(parsed.get("interrupted_assistant_text", "")).strip()[:400]
    interrupted_at_ms_raw = parsed.get("interrupted_at_ms")
    interrupted_at_ms: int | None = None
    if isinstance(interrupted_at_ms_raw, (int, float)):
        interrupted_at_ms = max(0, min(int(interrupted_at_ms_raw), 600_000))

    return {
        "barge_in": barge_in,
        "interrupted_assistant_text": interrupted_text,
        "interrupted_at_ms": interrupted_at_ms,
    }
