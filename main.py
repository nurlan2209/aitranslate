import os
import io
import json
import wave
import struct
import math
import logging
import time
import asyncio
import re
from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from fastapi.staticfiles import StaticFiles
from fastapi.responses import FileResponse
from dotenv import load_dotenv
from openai import AsyncOpenAI
from vosk import Model, KaldiRecognizer

load_dotenv()

LOG_FORMAT = "%(asctime)s | %(levelname)s | %(name)s | %(message)s"
LOG_DATEFMT = "%Y-%m-%d %H:%M:%S"


def configure_logging() -> None:
    formatter = logging.Formatter(LOG_FORMAT, LOG_DATEFMT)
    root_logger = logging.getLogger()

    if not root_logger.handlers:
        logging.basicConfig(level=logging.INFO, format=LOG_FORMAT, datefmt=LOG_DATEFMT)
    else:
        for handler in root_logger.handlers:
            handler.setFormatter(formatter)

    # Align uvicorn/httpx/app log formats so the timeline is comparable by seconds.
    for logger_name in ("uvicorn", "uvicorn.error", "uvicorn.access", "httpx", "translation-server"):
        named_logger = logging.getLogger(logger_name)
        for handler in named_logger.handlers:
            handler.setFormatter(formatter)


configure_logging()
logger = logging.getLogger("translation-server")

app = FastAPI()
app.mount("/static", StaticFiles(directory="static"), name="static")

# ── API Client ──
OPENAI_API_KEY = os.getenv("OPENAI_API_KEY")
TRANSLATION_MODEL = os.getenv("TRANSLATION_MODEL", "gpt-4o-mini")
REMOTE_STT_MODEL = os.getenv("REMOTE_STT_MODEL", "gpt-4o-transcribe")
KAZAKH_STT_ENGINE = os.getenv("KAZAKH_STT_ENGINE", "remote").lower()

if not OPENAI_API_KEY:
    logger.warning("OPENAI_API_KEY is not set. Translation requests will fail until you configure it in .env.")

client = AsyncOpenAI(api_key=OPENAI_API_KEY)

# ── Vosk Kazakh Model (loaded once at startup) ──
VOSK_MODEL_PATH = "vosk-model-small-kz-0.15"
vosk_kz_model = None

if os.path.isdir(VOSK_MODEL_PATH):
    logger.info(f"Loading Vosk kazakh model from '{VOSK_MODEL_PATH}'...")
    vosk_kz_model = Model(VOSK_MODEL_PATH)
    logger.info("✅ Vosk kazakh model loaded!")
else:
    logger.warning(f"⚠️  Vosk kazakh model not found at '{VOSK_MODEL_PATH}'")

# ──────────────────────────────────────────────
#  Hallucination Filter
# ──────────────────────────────────────────────
HALLUCINATION_PHRASES = {
    "thank you for watching", "thanks for watching",
    "thank you for listening", "thanks for listening",
    "thank you so much for watching",
    "please subscribe", "like and subscribe",
    "see you next time", "see you in the next video",
    "bye bye", "goodbye", "bye",
    "спасибо за просмотр", "подписывайтесь на канал",
    "до свидания", "субтитры", "субтитры от",
    "до следующего видео", "пока",
    "subtitles by", "translated by",
    "amara.org", "www.mooji.org",
    "you", "the end", "to be continued",
    "do zobaczenia w następnym filmiku",
    "do zobaczenia", "dziękuję za obejrzenie",
    "vielen dank fürs zuschauen", "bis zum nächsten mal",
    "...", ".", "",
}

# Strictly supported source languages
SUPPORTED_SOURCE_LANGS = {"kazakh", "russian", "english"}
EXPECTED_LANGUAGES = SUPPORTED_SOURCE_LANGS

SILENCE_RMS_THRESHOLD = int(os.getenv("SILENCE_RMS_THRESHOLD", "800"))
MIN_WORDS_TO_EMIT = int(os.getenv("MIN_WORDS_TO_EMIT", "1"))
REPEAT_EMIT_SECONDS = float(os.getenv("REPEAT_EMIT_SECONDS", "4"))
MIN_TEXT_LENGTH_TO_EMIT = int(os.getenv("MIN_TEXT_LENGTH_TO_EMIT", "6"))
MIN_ALPHA_CHARS_TO_EMIT = int(os.getenv("MIN_ALPHA_CHARS_TO_EMIT", "4"))
MIN_ALPHA_RATIO_TO_EMIT = float(os.getenv("MIN_ALPHA_RATIO_TO_EMIT", "0.55"))
KAZAKH_SPECIFIC_LETTERS = set("әіңғүұқөһ")
PROFANITY_PATTERNS = [
    r"\b(fuck|fucking|motherfucker|shit|bitch|asshole|bastard|cunt|dick|cock)\b",
    r"\b(бля|бляд|сук|сучк|хуй|хуе|хуйня|пизд|еба|ёба|наху|долбоеб)\w*\b",
    r"\b(boq|boqtyq|боқ|сік|сiк|shit|fuck)\w*\b",
]
PROFANITY_RE = re.compile("|".join(PROFANITY_PATTERNS), re.IGNORECASE)
SPOKEN_ABBREVIATION_REPLACEMENTS = [
    (re.compile(r"\b[эе]м\s+эн\s+ю\b", re.IGNORECASE), "MNU"),
    (re.compile(r"\bэмэню\b", re.IGNORECASE), "MNU"),
]


def compute_rms(pcm_bytes: bytes) -> float:
    """Compute RMS energy of raw 16-bit PCM audio."""
    if len(pcm_bytes) < 2:
        return 0.0
    num_samples = len(pcm_bytes) // 2
    samples = struct.unpack(f'<{num_samples}h', pcm_bytes[:num_samples * 2])
    if not samples:
        return 0.0
    sum_sq = sum(s * s for s in samples)
    return math.sqrt(sum_sq / num_samples)


def is_hallucination(text: str, detected_lang: str = "") -> bool:
    """Check if transcribed text is a known hallucination/noise phrase."""
    cleaned = text.strip().lower().rstrip('.!?,;:')
    if cleaned in HALLUCINATION_PHRASES:
        return True
    if len(cleaned) <= 2:
        return True
    if detected_lang and detected_lang not in EXPECTED_LANGUAGES:
        logger.info(f"Filtered unexpected language '{detected_lang}': '{text}'")
        return True
    return False


@app.get("/")
async def get():
    return FileResponse("static/index.html")


def transcribe_vosk_chunk(recognizer: KaldiRecognizer, pcm_data: bytes) -> str:
    """Feed one PCM chunk into an existing Vosk recognizer and return partial text."""
    try:
        recognizer.AcceptWaveform(pcm_data)
        partial = json.loads(recognizer.PartialResult())
        return partial.get("partial", "").strip()
    except Exception as e:
        logger.error(f"Vosk error: {e}")
        return ""


def extract_incremental_text(previous_text: str, current_text: str) -> str:
    """Return only the new tail if current text extends previous text."""
    prev_words = previous_text.split()
    curr_words = current_text.split()
    if not prev_words or not curr_words:
        return current_text
    if len(curr_words) <= len(prev_words):
        return current_text
    if curr_words[: len(prev_words)] == prev_words:
        return " ".join(curr_words[len(prev_words):]).strip()
    return current_text


def detect_language_from_text(text: str) -> str:
    """Heuristic language detection for kk/ru/en from transcript text."""
    lowered = (text or "").lower()
    if not lowered.strip():
        return "unknown"

    if any(ch in KAZAKH_SPECIFIC_LETTERS for ch in lowered):
        return "kazakh"

    latin_count = sum("a" <= ch <= "z" for ch in lowered)
    cyrillic_count = sum("а" <= ch <= "я" or ch == "ё" for ch in lowered)

    if latin_count > cyrillic_count and latin_count > 0:
        return "english"
    if cyrillic_count > 0:
        return "russian"
    return "unknown"


def normalize_lang_code(lang: str) -> str:
    value = (lang or "").lower()
    if value in {"kk", "kazakh"}:
        return "kazakh"
    if value in {"ru", "russian"}:
        return "russian"
    if value in {"en", "english"}:
        return "english"
    return value or "unknown"


def parse_glossary_terms(raw_terms) -> list[str]:
    if raw_terms is None:
        return []
    if isinstance(raw_terms, list):
        parts = [str(item) for item in raw_terms]
    else:
        raw = str(raw_terms)
        parts = re.split(r"[\n,;]+", raw)
    cleaned = []
    seen = set()
    for term in parts:
        value = re.sub(r"\s+", " ", str(term or "")).strip()
        if not value:
            continue
        lower = value.lower()
        if lower in seen:
            continue
        seen.add(lower)
        cleaned.append(value)
    return cleaned[:200]


def merged_glossary_terms(custom_terms: list[str] | None = None) -> list[str]:
    merged = []
    seen = set()
    for term in (custom_terms or []):
        value = re.sub(r"\s+", " ", str(term or "")).strip()
        if not value:
            continue
        lower = value.lower()
        if lower in seen:
            continue
        seen.add(lower)
        merged.append(value)
    return merged[:200]


def build_glossary_prompt(glossary_terms: list[str] | None) -> str:
    terms = merged_glossary_terms(glossary_terms)
    if not terms:
        return ""
    preview = ", ".join(terms[:60])
    return (
        "Preferred official terms and abbreviations. Use only when acoustically present, do not invent new terms. "
        "Preserve spelling exactly when they appear "
        f"or when ASR is close: {preview}"
    )


def lang_to_whisper_code(lang: str | None) -> str | None:
    normalized = normalize_lang_code(lang or "")
    if normalized == "russian":
        return "ru"
    if normalized == "english":
        return "en"
    if normalized == "kazakh":
        return "kk"
    return None


def pcm_to_wav(pcm_data: bytes, sample_rate: int = 16000, channels: int = 1, sample_width: int = 2) -> bytes:
    wav_buffer = io.BytesIO()
    with wave.open(wav_buffer, "wb") as wav_file:
        wav_file.setnchannels(channels)
        wav_file.setsampwidth(sample_width)
        wav_file.setframerate(sample_rate)
        wav_file.writeframes(pcm_data)
    wav_buffer.seek(0)
    return wav_buffer.read()


async def transcribe_whisper(
    wav_bytes: bytes,
    language_hint: str | None = None,
    glossary_terms: list[str] | None = None,
) -> dict:
    """Transcribe with OpenAI audio model; optionally pass source-language hint (ru/en/kk)."""
    try:
        audio_file = io.BytesIO(wav_bytes)
        audio_file.name = "audio.wav"
        request_args = {
            "model": REMOTE_STT_MODEL,
            "file": audio_file,
            "response_format": "json",
        }
        glossary_prompt = build_glossary_prompt(glossary_terms)
        if glossary_prompt:
            request_args["prompt"] = glossary_prompt
        if language_hint:
            request_args["language"] = language_hint
        transcript = await client.audio.transcriptions.create(**request_args)
        text = transcript.text.strip() if transcript.text else ""
        raw_language = getattr(transcript, "language", None)
        language = normalize_lang_code(raw_language) if raw_language else "unknown"
        return {"text": text, "language": language}
    except Exception as e:
        logger.error(f"Whisper error: {e}")
        return {"text": "", "language": "unknown"}


def sanitize_text_for_business(text: str) -> str:
    cleaned = PROFANITY_RE.sub("", str(text or ""))
    cleaned = re.sub(r"\s{2,}", " ", cleaned)
    cleaned = re.sub(r"\s+([,.;!?])", r"\1", cleaned)
    return cleaned.strip()


def normalize_spoken_abbreviations(text: str) -> str:
    value = str(text or "")
    for pattern, replacement in SPOKEN_ABBREVIATION_REPLACEMENTS:
        value = pattern.sub(replacement, value)
    return value


def sanitize_translations_for_business(translations: dict) -> dict:
    return {
        "RU": sanitize_text_for_business(translations.get("RU", "")),
        "EN": sanitize_text_for_business(translations.get("EN", "")),
        "KK": sanitize_text_for_business(translations.get("KK", "")),
    }


def is_low_quality_text(text: str) -> bool:
    """Filter out click/noise artifacts that look like accidental pseudo-words."""
    value = (text or "").strip()
    if not value:
        return True
    if len(value) < MIN_TEXT_LENGTH_TO_EMIT:
        return True

    alpha_chars = [ch for ch in value if ch.isalpha()]
    if len(alpha_chars) < MIN_ALPHA_CHARS_TO_EMIT:
        return True

    alpha_ratio = len(alpha_chars) / max(len(value), 1)
    if alpha_ratio < MIN_ALPHA_RATIO_TO_EMIT:
        return True
    return False


async def translate_text(text: str, detected_lang: str, glossary_terms: list[str] | None = None) -> dict:
    """Translate text into RU, EN, KK using OpenAI."""
    try:
        if not OPENAI_API_KEY:
            logger.error("OPENAI_API_KEY is missing.")
            return {"RU": text, "EN": text, "KK": text}

        lang_names = {
            "russian": "Russian", "english": "English",
            "kazakh": "Kazakh", "kk": "Kazakh"
        }
        source_name = lang_names.get(detected_lang, detected_lang)
        glossary_prompt = build_glossary_prompt(glossary_terms)

        response = await client.chat.completions.create(
            model=TRANSLATION_MODEL,
            temperature=0.2,
            response_format={"type": "json_object"},
            messages=[
                {
                    "role": "system",
                    "content": (
                        "You are a real-time conference translator. "
                        f"The source text is in {source_name}. "
                        "First, rewrite the source into a clean, grammatically correct sentence with natural punctuation, "
                        "while preserving meaning and without adding facts. "
                        "Remove filler words and disfluencies (e.g., 'ээ', 'эм', repetitions) unless they change meaning. "
                        "Translate it accurately into three languages: "
                        "Russian (RU), English (EN), and Kazakh (KK). "
                        "For Kazakh, use proper Kazakh Cyrillic script. "
                        "Strict policy: profanity/obscenity/insults are forbidden in any language. "
                        "If source contains such words, replace with neutral business-safe wording. "
                        f"{glossary_prompt} "
                        "Keep each translation concise for subtitle display. "
                        "Return ONLY valid JSON: {\"RU\": \"...\", \"EN\": \"...\", \"KK\": \"...\"}"
                    ),
                },
                {"role": "user", "content": f"Translate: \"{text}\""},
            ],
        )
        content = response.choices[0].message.content or "{}"
        parsed = json.loads(content)
        parsed = {
            "RU": str(parsed.get("RU") or text),
            "EN": str(parsed.get("EN") or text),
            "KK": str(parsed.get("KK") or text),
        }
        return sanitize_translations_for_business(parsed)
    except asyncio.CancelledError:
        raise
    except Exception as e:
        logger.error(f"Translation error: {e}")
        safe = sanitize_text_for_business(text)
        return {"RU": safe, "EN": safe, "KK": safe}


# ──────────────────────────────────────────────
#  WebSocket Handler
# ──────────────────────────────────────────────
@app.websocket("/ws/audio")
async def websocket_endpoint(websocket: WebSocket):
    await websocket.accept()
    logger.info("WebSocket client connected")
    recognizer = None
    language_mode = "auto"
    manual_source_lang = "kazakh"
    custom_glossary_terms: list[str] = []
    stt_warning = None

    if vosk_kz_model is not None:
        try:
            recognizer = KaldiRecognizer(vosk_kz_model, 16000)
        except Exception as e:
            logger.error(f"Failed to initialize Vosk recognizer: {e}")
            recognizer = None
    else:
        stt_warning = "Vosk kazakh model is missing. Kazakh STT unavailable."

    last_emitted_text = ""
    last_emit_time = 0.0
    latest_job_id = 0
    translation_task = None

    async def run_translation_job(
        job_id: int,
        source_text: str,
        source_lang: str,
        stt_ms: float,
        rms_value: float,
        glossary_snapshot: list[str],
    ) -> None:
        try:
            translate_started = time.perf_counter()
            translations = await translate_text(source_text, source_lang, glossary_snapshot)
            translate_ms = (time.perf_counter() - translate_started) * 1000.0
            total_ms = stt_ms + translate_ms

            if job_id != latest_job_id:
                return

            logger.info(f"[pipeline] total_ms={total_ms:.0f} stt_ms={stt_ms:.0f} translate_ms={translate_ms:.0f} rms={rms_value:.0f}")
            await websocket.send_json({
                "type": "translated",
                "original": source_text,
                "detected_language": source_lang,
                "translations": translations
            })
        except asyncio.CancelledError:
            logger.debug("Translation job cancelled")
        except Exception as e:
            logger.error(f"Translation job error: {e}")

    def current_stt_label() -> str:
        if language_mode == "manual" and manual_source_lang == "kazakh":
            if KAZAKH_STT_ENGINE == "vosk" and recognizer is not None:
                return "kazakh-vosk"
            return REMOTE_STT_MODEL
        return REMOTE_STT_MODEL

    try:
        await websocket.send_json({
            "type": "settings_state",
            "language_mode": language_mode,
            "manual_source_lang": manual_source_lang,
            "stt_model_lang": current_stt_label(),
            "available_stt_models": ["kazakh-vosk", REMOTE_STT_MODEL],
            "custom_glossary_terms": custom_glossary_terms,
            "warning": stt_warning,
        })

        while True:
            incoming = await websocket.receive()
            if incoming.get("type") == "websocket.disconnect":
                logger.info("Client disconnected")
                if translation_task and not translation_task.done():
                    translation_task.cancel()
                break

            if incoming.get("text") is not None:
                try:
                    payload = json.loads(incoming["text"])
                except Exception:
                    continue

                msg_type = payload.get("type")
                if msg_type == "set_language_mode":
                    requested_mode = str(payload.get("mode", "auto")).lower()
                    requested_lang = str(payload.get("manual_lang", manual_source_lang)).lower()
                    if requested_mode in {"auto", "manual"}:
                        language_mode = requested_mode
                    if requested_lang in SUPPORTED_SOURCE_LANGS:
                        manual_source_lang = normalize_lang_code(requested_lang)

                    await websocket.send_json({
                        "type": "settings_state",
                        "language_mode": language_mode,
                        "manual_source_lang": manual_source_lang,
                        "stt_model_lang": current_stt_label(),
                        "available_stt_models": ["kazakh-vosk", REMOTE_STT_MODEL],
                        "custom_glossary_terms": custom_glossary_terms,
                        "warning": stt_warning,
                    })
                elif msg_type == "set_glossary":
                    custom_glossary_terms = parse_glossary_terms(payload.get("terms"))
                    await websocket.send_json({
                        "type": "settings_state",
                        "language_mode": language_mode,
                        "manual_source_lang": manual_source_lang,
                        "stt_model_lang": current_stt_label(),
                        "available_stt_models": ["kazakh-vosk", REMOTE_STT_MODEL],
                        "custom_glossary_terms": custom_glossary_terms,
                        "warning": stt_warning,
                    })
                elif msg_type == "get_settings":
                    await websocket.send_json({
                        "type": "settings_state",
                        "language_mode": language_mode,
                        "manual_source_lang": manual_source_lang,
                        "stt_model_lang": current_stt_label(),
                        "available_stt_models": ["kazakh-vosk", REMOTE_STT_MODEL],
                        "custom_glossary_terms": custom_glossary_terms,
                        "warning": stt_warning,
                    })
                continue

            pcm_data = incoming.get("bytes")
            if pcm_data is None:
                continue

            if len(pcm_data) < 3200:
                continue

            # ── Silence Detection ──
            rms = compute_rms(pcm_data)
            if rms < SILENCE_RMS_THRESHOLD:
                continue

            stt_started = time.perf_counter()
            detected_lang = "kazakh"
            text = ""
            active_glossary_terms = merged_glossary_terms(custom_glossary_terms)

            if language_mode == "manual":
                if manual_source_lang == "kazakh":
                    if KAZAKH_STT_ENGINE == "vosk" and recognizer is not None:
                        text = transcribe_vosk_chunk(recognizer, pcm_data)
                        detected_lang = "kazakh"
                    else:
                        wav_bytes = pcm_to_wav(pcm_data)
                        whisper = await transcribe_whisper(
                            wav_bytes,
                            language_hint=lang_to_whisper_code("kazakh"),
                            glossary_terms=active_glossary_terms,
                        )
                        text = whisper["text"]
                        detected_lang = "kazakh"
                else:
                    wav_bytes = pcm_to_wav(pcm_data)
                    whisper = await transcribe_whisper(
                        wav_bytes,
                        language_hint=lang_to_whisper_code(manual_source_lang),
                        glossary_terms=active_glossary_terms,
                    )
                    text = whisper["text"]
                    detected_lang = manual_source_lang
            else:
                wav_bytes = pcm_to_wav(pcm_data)
                whisper = await transcribe_whisper(wav_bytes, glossary_terms=active_glossary_terms)
                text = whisper["text"]
                stt_detected_lang = normalize_lang_code(whisper["language"])
                if stt_detected_lang in SUPPORTED_SOURCE_LANGS:
                    detected_lang = stt_detected_lang
                elif stt_detected_lang == "unknown":
                    detected_lang = detect_language_from_text(text)
                else:
                    logger.info(
                        f"Skipped chunk due to unsupported STT language '{stt_detected_lang}': '{text[:80]}'"
                    )
                    continue
                if detected_lang == "kazakh" and KAZAKH_STT_ENGINE == "vosk" and recognizer is not None:
                    vosk_text = transcribe_vosk_chunk(recognizer, pcm_data)
                    if vosk_text:
                        text = vosk_text

            stt_ms = (time.perf_counter() - stt_started) * 1000.0
            now_ts = time.perf_counter()

            if not text:
                continue
            text = normalize_spoken_abbreviations(text)
            word_count = len(text.split())
            if word_count < MIN_WORDS_TO_EMIT:
                continue
            same_as_last = text == last_emitted_text
            if same_as_last and (now_ts - last_emit_time) < REPEAT_EMIT_SECONDS:
                continue

            incremental_text = extract_incremental_text(last_emitted_text, text)
            incremental_text = normalize_spoken_abbreviations(incremental_text)
            if len(incremental_text.split()) < MIN_WORDS_TO_EMIT:
                continue
            if is_low_quality_text(incremental_text):
                logger.debug(f"Filtered low-quality text: '{incremental_text}'")
                continue

            # Strict text-level language gate: process only kk/ru/en scripts.
            heuristic_lang = detect_language_from_text(incremental_text)
            if heuristic_lang == "unknown":
                logger.info(f"Skipped unsupported-script text: '{incremental_text}'")
                continue
            if language_mode == "manual" and heuristic_lang in SUPPORTED_SOURCE_LANGS and heuristic_lang != detected_lang:
                logger.info(
                    f"Manual lang '{detected_lang}' overridden by text heuristic '{heuristic_lang}': '{incremental_text}'"
                )
                detected_lang = heuristic_lang

            detected_lang = normalize_lang_code(detected_lang)
            if detected_lang not in SUPPORTED_SOURCE_LANGS:
                logger.info(f"Skipped unsupported detected language '{detected_lang}': '{incremental_text}'")
                continue

            last_emitted_text = text
            last_emit_time = now_ts

            # ── Step 3: Hallucination Filter ──
            if not incremental_text or is_hallucination(incremental_text, detected_lang):
                logger.debug(f"Filtered: [{detected_lang}] '{incremental_text}'")
                continue

            logger.info(
                f"[{detected_lang}] {incremental_text} | words={len(incremental_text.split())} | rms={rms:.0f} | stt_ms={stt_ms:.0f}"
            )

            # Send recognized source text immediately for low-latency UI updates.
            await websocket.send_json({
                "type": "recognized",
                "original": sanitize_text_for_business(incremental_text),
                "detected_language": detected_lang,
            })

            # ── Step 4: Translate (latest-wins; cancel stale in-flight requests) ──
            latest_job_id += 1
            if translation_task and not translation_task.done():
                translation_task.cancel()
            translation_task = asyncio.create_task(
                run_translation_job(
                    latest_job_id,
                    incremental_text,
                    detected_lang,
                    stt_ms,
                    rms,
                    active_glossary_terms,
                )
            )

    except WebSocketDisconnect:
        logger.info("Client disconnected")
        if translation_task and not translation_task.done():
            translation_task.cancel()
    except Exception as e:
        logger.error(f"WebSocket error: {e}")
        if translation_task and not translation_task.done():
            translation_task.cancel()
        try:
            await websocket.close()
        except:
            pass
