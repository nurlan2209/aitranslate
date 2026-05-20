# Realtime Subtitles: Rust + gRPC

Realtime STT/translation service for conference subtitles.

Current backend stack:
- Rust (`tokio`, `tonic`, `axum`)
- gRPC bidirectional stream (`RealtimePipeline.Stream`)
- WebSocket gateway `/ws/audio` for existing browser client
- OpenRouter API (OpenAI-compatible):
  - STT: `/audio/transcriptions` (`REMOTE_STT_MODEL`)
  - Translation: `/chat/completions` (`TRANSLATION_MODEL`)

## Run

1. Install Rust toolchain and protobuf compiler (`protoc`):
```bash
brew install rust protobuf
```

2. Create `.env` in project root:
```dotenv
OPENROUTER_API_KEY=sk-or-...
OPENROUTER_BASE_URL=https://openrouter.ai/api/v1
TRANSLATION_MODEL=openai/gpt-4o-mini
REMOTE_STT_MODEL=openai/gpt-4o-transcribe
KAZAKH_STT_ENGINE=remote

SILENCE_RMS_THRESHOLD=800
MIN_WORDS_TO_EMIT=1
REPEAT_EMIT_SECONDS=4
MIN_TEXT_LENGTH_TO_EMIT=6
MIN_ALPHA_CHARS_TO_EMIT=4
MIN_ALPHA_RATIO_TO_EMIT=0.55

GRPC_ADDR=127.0.0.1:50051
HTTP_ADDR=127.0.0.1:8000
```

3. Build and run:
```bash
cargo run
```

4. Open:
```text
http://127.0.0.1:8000
```
History UI:
```text
http://127.0.0.1:8000/history
```

## Endpoints

- HTTP/UI: `GET /`
- History UI: `GET /history`
- Static: `GET /static/*`
- WebSocket audio ingress: `GET /ws/audio`
- History API: `GET /api/history`, `GET /api/history/sessions`, `POST /api/history/clear`
- gRPC: `RealtimePipeline.Stream` on `GRPC_ADDR`

## Notes

- Frontend protocol (`settings_state`, `recognized`, `translated`) is preserved.
- Only source languages are accepted: Kazakh, Russian, English.
- Unsupported scripts/languages are filtered before translation step.
- Glossary terms are used as STT/translation prompt hints.
- Conversation history is stored locally in `data/history.jsonl` (configurable via `HISTORY_FILE`).
- History is grouped by recording sessions (`Start` -> `Stop`) with session names shown as date/time in UI.
