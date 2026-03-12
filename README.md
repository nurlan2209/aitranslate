# Realtime Subtitles: Rust + gRPC

Realtime STT/translation service for conference subtitles.

Current backend stack:
- Rust (`tokio`, `tonic`, `axum`)
- gRPC bidirectional stream (`RealtimePipeline.Stream`)
- WebSocket gateway `/ws/audio` for existing browser client
- OpenAI API:
  - STT: `/v1/audio/transcriptions` (`REMOTE_STT_MODEL`)
  - Translation: `/v1/chat/completions` (`TRANSLATION_MODEL`)

## Run

1. Install Rust toolchain and protobuf compiler (`protoc`):
```bash
brew install rust protobuf
```

2. Create `.env` in project root:
```dotenv
OPENAI_API_KEY=sk-...
TRANSLATION_MODEL=gpt-4o-mini
REMOTE_STT_MODEL=gpt-4o-transcribe
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

## Endpoints

- HTTP/UI: `GET /`
- Static: `GET /static/*`
- WebSocket audio ingress: `GET /ws/audio`
- gRPC: `RealtimePipeline.Stream` on `GRPC_ADDR`

## Notes

- Frontend protocol (`settings_state`, `recognized`, `translated`) is preserved.
- Only source languages are accepted: Kazakh, Russian, English.
- Unsupported scripts/languages are filtered before translation step.
- Glossary terms are used as STT/translation prompt hints.
