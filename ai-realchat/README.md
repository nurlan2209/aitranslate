# AI RealChat MVP

Minimal assistant MVP:
- `frontend/` - Next.js UI with two modes:
  - standard text chat
  - realtime voice (record + spoken answer)
- `backend/` - FastAPI API for text and voice routes (STT -> LLM -> TTS)

Supported conversation languages:
- Russian
- Kazakh
- English

UI language can be switched manually: `RU / KZ / EN`.

Extra conversation logic:
- user can interrupt assistant playback (`barge-in`);
- interruption metadata is passed to backend for better continuation handling;
- short/incomplete user utterances are treated as clarification candidates.

## UX

- `Chat` mode: send text, receive short text response.
- `Realtime voice` mode: record voice, receive text + spoken audio response.
- Shared dialog history between both modes.

## Architecture

1. User taps `Start speaking`
2. Browser records audio with `MediaRecorder`
3. Audio is sent to FastAPI `/api/voice`
4. Backend transcribes audio (`gpt-4o-mini-transcribe`)
5. Backend asks LLM (`gpt-4o-mini`)
6. Backend generates speech (`gpt-4o-mini-tts`)
7. Frontend receives text + audio, shows chat, plays audio

## Backend setup

```bash
cd backend
python3.10 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
cp .env.example .env
```

Fill `.env`:

```dotenv
OPENAI_API_KEY=sk-...
OPENAI_STT_MODEL=gpt-4o-mini-transcribe
OPENAI_CHAT_MODEL=gpt-4o-mini
OPENAI_TTS_MODEL=gpt-4o-mini-tts
OPENAI_TTS_VOICE=alloy
OPENAI_TTS_FORMAT=mp3
FRONTEND_ORIGIN=http://localhost:3040
```

Run backend:

```bash
uvicorn main:app --reload --host 0.0.0.0 --port 8090
```

Health check:

```bash
curl http://localhost:8090/health
```

## Frontend setup

```bash
cd frontend
npm install
cp .env.local.example .env.local
```

Optional `.env.local`:

```dotenv
NEXT_PUBLIC_BACKEND_URL=http://localhost:8090
```

Run frontend:

```bash
npm run dev
```

Open:

```text
http://localhost:3040
```

## API

### `POST /api/voice`

Multipart form fields:
- `file` - recorded audio file
- `history` - JSON array of previous messages (optional)
- `turn_meta` - JSON object with turn context (optional), for example:
  - `barge_in` (`true/false`)
  - `interrupted_assistant_text` (string)
  - `interrupted_at_ms` (integer)

Response JSON:

```json
{
  "transcript": "...",
  "response_text": "...",
  "audio_base64": "...",
  "audio_mime_type": "audio/mpeg"
}
```

### `POST /api/chat`

Request JSON:

```json
{
  "message": "Hello",
  "history": [{"role": "user", "content": "Hi"}],
  "turn_meta": {"barge_in": false},
  "speak": false
}
```

Response JSON:

```json
{
  "response_text": "Hi! How can I help?"
}
```

If `speak=true`, response may include:
- `audio_base64`
- `audio_mime_type`

## Tests

Detailed scenarios are in `TEST_SCENARIOS.md`.

Run backend tests:

```bash
cd backend
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements-dev.txt
pytest -q
```
