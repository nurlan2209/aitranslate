# AITranslate (Python + Next.js)

Проект переведен на стек:
- backend: Python (FastAPI)
- frontend: Next.js

## Структура

- backend/
- frontend/
- scripts/
- docker-compose.yml
- README.md

## Быстрый старт (Docker)

1. Скопируйте переменные окружения:
   - `backend/.env.example` -> `backend/.env`
   - `frontend/.env.local.example` -> `frontend/.env.local` (опционально)
2. Запустите:
   - `docker compose up --build`
3. Откройте:
   - Frontend: `http://localhost:3040`
   - Backend health: `http://localhost:8090/health`

## Локальный запуск без Docker

### Backend

```bash
cd backend
python -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt
uvicorn main:app --reload --host 0.0.0.0 --port 8090
```

### Frontend

```bash
cd frontend
npm install
npm run dev
```

## Скрипты

- `scripts/dev-up.sh` - запуск docker compose с билдом
- `scripts/dev-down.sh` - остановка docker compose
