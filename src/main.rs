use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, http::StatusCode};
use futures_util::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};
use tower_http::services::ServeDir;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

pub mod realtime {
    tonic::include_proto!("realtime");
}

use realtime::client_message::Payload as ClientPayload;
use realtime::realtime_pipeline_client::RealtimePipelineClient;
use realtime::realtime_pipeline_server::{RealtimePipeline, RealtimePipelineServer};
use realtime::server_message::Payload as ServerPayload;
use realtime::{
    AudioChunk, ClientMessage, ErrorEvent, GetSettings, LanguageMode, RecognizedEvent,
    ServerMessage, SetGlossary, SetLanguageMode, SettingsState, SourceLanguage, TranslatedEvent,
};

const HALLUCINATION_PHRASES: &[&str] = &[
    "thank you for watching",
    "thanks for watching",
    "thank you for listening",
    "thanks for listening",
    "thank you so much for watching",
    "please subscribe",
    "like and subscribe",
    "see you next time",
    "see you in the next video",
    "bye bye",
    "goodbye",
    "bye",
    "спасибо за просмотр",
    "подписывайтесь на канал",
    "до свидания",
    "субтитры",
    "субтитры от",
    "до следующего видео",
    "пока",
    "subtitles by",
    "translated by",
    "amara.org",
    "www.mooji.org",
    "you",
    "the end",
    "to be continued",
    "do zobaczenia w następnym filmiku",
    "do zobaczenia",
    "dziękuję za obejrzenie",
    "vielen dank fürs zuschauen",
    "bis zum nächsten mal",
    "...",
    ".",
    "",
];

static HALLUCINATION_SET: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HALLUCINATION_PHRASES
        .iter()
        .copied()
        .collect::<HashSet<_>>()
});

static PROFANITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(fuck|fucking|motherfucker|shit|bitch|asshole|bastard|cunt|dick|cock|бля|бляд|сук|сучк|хуй|хуе|хуйня|пизд|еба|ёба|наху|долбоеб|boq|boqtyq|боқ|сік|сiк)\w*\b",
    )
    .expect("valid profanity regex")
});
static MULTISPACE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s{2,}").expect("valid multispace regex"));
static SPACE_PUNCT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s+([,.;!?])").expect("valid punctuation regex"));

static SPOKEN_MNU_RE_1: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b[эе]м\s+эн\s+ю\b").expect("valid spoken mnu regex"));
static SPOKEN_MNU_RE_2: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bэмэню\b").expect("valid spoken mnu compact regex"));
static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct AppConfig {
    openai_api_key: String,
    translation_model: String,
    remote_stt_model: String,
    kazakh_stt_engine: String,
    silence_rms_threshold: f64,
    min_words_to_emit: usize,
    repeat_emit_seconds: f64,
    min_text_length_to_emit: usize,
    min_alpha_chars_to_emit: usize,
    min_alpha_ratio_to_emit: f64,
    grpc_addr: SocketAddr,
    http_addr: SocketAddr,
    grpc_endpoint: String,
    history_file: String,
    history_max_entries: usize,
}

impl AppConfig {
    fn from_env() -> Result<Self, String> {
        dotenvy::dotenv().ok();
        let openai_api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "OPENAI_API_KEY is required in .env".to_string())?;
        let translation_model =
            std::env::var("TRANSLATION_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
        let remote_stt_model =
            std::env::var("REMOTE_STT_MODEL").unwrap_or_else(|_| "gpt-4o-transcribe".to_string());
        let kazakh_stt_engine =
            std::env::var("KAZAKH_STT_ENGINE").unwrap_or_else(|_| "remote".to_string());
        let silence_rms_threshold = parse_env_f64("SILENCE_RMS_THRESHOLD", 800.0)?;
        let min_words_to_emit = parse_env_usize("MIN_WORDS_TO_EMIT", 1)?;
        let repeat_emit_seconds = parse_env_f64("REPEAT_EMIT_SECONDS", 4.0)?;
        let min_text_length_to_emit = parse_env_usize("MIN_TEXT_LENGTH_TO_EMIT", 6)?;
        let min_alpha_chars_to_emit = parse_env_usize("MIN_ALPHA_CHARS_TO_EMIT", 4)?;
        let min_alpha_ratio_to_emit = parse_env_f64("MIN_ALPHA_RATIO_TO_EMIT", 0.55)?;
        let grpc_addr = parse_env_addr("GRPC_ADDR", "127.0.0.1:50051")?;
        let http_addr = parse_env_addr("HTTP_ADDR", "127.0.0.1:8000")?;
        let grpc_endpoint =
            std::env::var("GRPC_ENDPOINT").unwrap_or_else(|_| format!("http://{}", grpc_addr));
        let history_file =
            std::env::var("HISTORY_FILE").unwrap_or_else(|_| "data/history.jsonl".to_string());
        let history_max_entries = parse_env_usize("HISTORY_MAX_ENTRIES", 5000)?;

        Ok(Self {
            openai_api_key,
            translation_model,
            remote_stt_model,
            kazakh_stt_engine,
            silence_rms_threshold,
            min_words_to_emit,
            repeat_emit_seconds,
            min_text_length_to_emit,
            min_alpha_chars_to_emit,
            min_alpha_ratio_to_emit,
            grpc_addr,
            http_addr,
            grpc_endpoint,
            history_file,
            history_max_entries,
        })
    }
}

#[derive(Clone)]
struct PipelineService {
    cfg: Arc<AppConfig>,
    http: reqwest::Client,
    history: Arc<HistoryStore>,
}

struct SessionState {
    session_id: String,
    session_started_ms: i64,
    language_mode: LanguageMode,
    manual_source_lang: SourceLanguage,
    custom_glossary_terms: Vec<String>,
    last_emitted_text: String,
    last_emit_time: Option<Instant>,
    latest_job_id: Arc<AtomicU64>,
    translation_task: Option<JoinHandle<()>>,
}

impl SessionState {
    fn new() -> Self {
        let session_started_ms = now_unix_ms();
        let seq = SESSION_SEQ.fetch_add(1, Ordering::SeqCst);
        let session_id = format!("session-{session_started_ms}-{seq}");
        Self {
            session_id,
            session_started_ms,
            language_mode: LanguageMode::Auto,
            manual_source_lang: SourceLanguage::Kazakh,
            custom_glossary_terms: Vec::new(),
            last_emitted_text: String::new(),
            last_emit_time: None,
            latest_job_id: Arc::new(AtomicU64::new(0)),
            translation_task: None,
        }
    }
}

#[derive(Clone)]
struct HttpState {
    grpc_endpoint: String,
    history: Arc<HistoryStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntry {
    id: u64,
    ts_ms: i64,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    session_started_ms: i64,
    source_lang: String,
    original: String,
    ru: String,
    en: String,
    kk: String,
}

#[derive(Debug)]
struct HistoryStore {
    file_path: PathBuf,
    max_entries: usize,
    next_id: AtomicU64,
    entries: RwLock<VecDeque<HistoryEntry>>,
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct HistoryListResponse {
    items: Vec<HistoryEntry>,
}

#[derive(Debug, Deserialize)]
struct HistorySessionsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct HistorySessionSummary {
    session_id: String,
    session_started_ms: i64,
    first_ts_ms: i64,
    last_ts_ms: i64,
    messages: usize,
}

#[derive(Debug, Serialize)]
struct HistorySessionsResponse {
    sessions: Vec<HistorySessionSummary>,
}

#[derive(Debug, Deserialize)]
struct OpenAiTranscriptionResponse {
    text: Option<String>,
    language: Option<String>,
}

#[derive(Debug)]
struct TranscriptionResult {
    text: String,
    language_raw: String,
}

#[derive(Debug)]
struct TranslationTriple {
    ru: String,
    en: String,
    kk: String,
}

impl HistoryStore {
    async fn load(file_path: PathBuf, max_entries: usize) -> Self {
        let mut entries = VecDeque::new();
        let mut next_id = 1u64;

        if let Ok(raw) = tokio::fs::read_to_string(&file_path).await {
            for line in raw.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(mut entry) = serde_json::from_str::<HistoryEntry>(line) {
                    if entry.session_id.trim().is_empty() {
                        entry.session_id = format!("legacy-{}", entry.id);
                    }
                    if entry.session_started_ms <= 0 {
                        entry.session_started_ms = entry.ts_ms;
                    }
                    next_id = next_id.max(entry.id + 1);
                    entries.push_back(entry);
                    while entries.len() > max_entries {
                        entries.pop_front();
                    }
                }
            }
        }

        Self {
            file_path,
            max_entries,
            next_id: AtomicU64::new(next_id),
            entries: RwLock::new(entries),
        }
    }

    async fn append(
        &self,
        source_lang: SourceLanguage,
        original: String,
        translation: &TranslationTriple,
        session_id: &str,
        session_started_ms: i64,
    ) -> Result<(), String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let ts_ms = now_unix_ms();
        let entry = HistoryEntry {
            id,
            ts_ms,
            session_id: session_id.to_string(),
            session_started_ms,
            source_lang: source_to_ui(source_lang).to_string(),
            original,
            ru: translation.ru.clone(),
            en: translation.en.clone(),
            kk: translation.kk.clone(),
        };

        {
            let mut lock = self.entries.write().await;
            lock.push_back(entry.clone());
            while lock.len() > self.max_entries {
                lock.pop_front();
            }
        }

        if let Some(parent) = self.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)
            .await
            .map_err(|e| e.to_string())?;
        let line = format!(
            "{}\n",
            serde_json::to_string(&entry).map_err(|e| e.to_string())?
        );
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list(&self, limit: usize, session_id: Option<&str>) -> Vec<HistoryEntry> {
        let lock = self.entries.read().await;
        let mut items: Vec<HistoryEntry> = lock
            .iter()
            .filter(|item| {
                if let Some(want) = session_id {
                    item.session_id == want
                } else {
                    true
                }
            })
            .cloned()
            .collect();
        if items.len() > limit {
            let drain = items.len() - limit;
            items.drain(0..drain);
        }
        items
    }

    async fn sessions(&self, limit: usize) -> Vec<HistorySessionSummary> {
        let lock = self.entries.read().await;
        let mut grouped: HashMap<String, HistorySessionSummary> = HashMap::new();

        for item in lock.iter() {
            let sid = if item.session_id.trim().is_empty() {
                format!("legacy-{}", item.id)
            } else {
                item.session_id.clone()
            };
            let started_ms = if item.session_started_ms > 0 {
                item.session_started_ms
            } else {
                item.ts_ms
            };

            if let Some(existing) = grouped.get_mut(&sid) {
                existing.messages += 1;
                if item.ts_ms < existing.first_ts_ms {
                    existing.first_ts_ms = item.ts_ms;
                }
                if item.ts_ms > existing.last_ts_ms {
                    existing.last_ts_ms = item.ts_ms;
                }
                if started_ms < existing.session_started_ms {
                    existing.session_started_ms = started_ms;
                }
            } else {
                grouped.insert(
                    sid.clone(),
                    HistorySessionSummary {
                        session_id: sid,
                        session_started_ms: started_ms,
                        first_ts_ms: item.ts_ms,
                        last_ts_ms: item.ts_ms,
                        messages: 1,
                    },
                );
            }
        }

        let mut sessions: Vec<HistorySessionSummary> = grouped.into_values().collect();
        sessions.sort_by(|a, b| {
            b.session_started_ms
                .cmp(&a.session_started_ms)
                .then_with(|| b.last_ts_ms.cmp(&a.last_ts_ms))
        });
        if sessions.len() > limit {
            sessions.truncate(limit);
        }
        sessions
    }

    async fn clear(&self) -> Result<(), String> {
        {
            let mut lock = self.entries.write().await;
            lock.clear();
        }
        self.next_id.store(1, Ordering::SeqCst);
        if let Some(parent) = self.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&self.file_path, b"")
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let cfg = Arc::new(AppConfig::from_env().map_err(|e| {
        error!("{e}");
        e
    })?);

    if cfg.kazakh_stt_engine.eq_ignore_ascii_case("vosk") {
        warn!("KAZAKH_STT_ENGINE=vosk is not implemented in Rust backend, using remote STT.");
    }

    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;

    let history_store = Arc::new(
        HistoryStore::load(
            PathBuf::from(cfg.history_file.clone()),
            cfg.history_max_entries,
        )
        .await,
    );

    let grpc_service = PipelineService {
        cfg: cfg.clone(),
        http: http.clone(),
        history: history_store.clone(),
    };

    let grpc_addr = cfg.grpc_addr;
    let http_addr = cfg.http_addr;
    info!("Starting gRPC on {}", grpc_addr);
    info!("Starting HTTP/WebSocket on {}", http_addr);

    let grpc_server = async move {
        Server::builder()
            .add_service(RealtimePipelineServer::new(grpc_service))
            .serve(grpc_addr)
            .await
    };

    let app_state = Arc::new(HttpState {
        grpc_endpoint: cfg.grpc_endpoint.clone(),
        history: history_store.clone(),
    });
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/history", get(history_handler))
        .route("/api/history", get(api_history_handler))
        .route("/api/history/sessions", get(api_history_sessions_handler))
        .route("/api/history/clear", post(api_history_clear_handler))
        .route("/ws/audio", get(ws_audio_handler))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(app_state);
    let listener = tokio::net::TcpListener::bind(http_addr).await?;
    let http_server = async move { axum::serve(listener, app).await };

    tokio::select! {
        res = grpc_server => {
            if let Err(err) = res {
                error!("gRPC server failed: {err}");
            }
        }
        res = http_server => {
            if let Err(err) = res {
                error!("HTTP server failed: {err}");
            }
        }
    }

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

async fn index_handler() -> impl IntoResponse {
    match tokio::fs::read("static/index.html").await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn history_handler() -> impl IntoResponse {
    match tokio::fs::read("static/history.html").await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "history.html not found").into_response(),
    }
}

async fn api_history_handler(
    State(state): State<Arc<HttpState>>,
    Query(query): Query<HistoryQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(300).clamp(1, 2000);
    let items = state.history.list(limit, query.session_id.as_deref()).await;
    Json(HistoryListResponse { items })
}

async fn api_history_sessions_handler(
    State(state): State<Arc<HttpState>>,
    Query(query): Query<HistorySessionsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(200).clamp(1, 2000);
    let sessions = state.history.sessions(limit).await;
    Json(HistorySessionsResponse { sessions })
}

async fn api_history_clear_handler(State(state): State<Arc<HttpState>>) -> impl IntoResponse {
    match state.history.clear().await {
        Ok(_) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
    }
}

async fn ws_audio_handler(
    State(state): State<Arc<HttpState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: Arc<HttpState>) {
    let mut grpc_client = match RealtimePipelineClient::connect(state.grpc_endpoint.clone()).await {
        Ok(client) => client,
        Err(err) => {
            error!("Failed to connect local gRPC client: {err}");
            return;
        }
    };

    let (tx_in, rx_in) = mpsc::channel::<ClientMessage>(64);
    let outbound = ReceiverStream::new(rx_in);
    let response = match grpc_client.stream(Request::new(outbound)).await {
        Ok(resp) => resp,
        Err(err) => {
            error!("Failed to open gRPC stream: {err}");
            return;
        }
    };

    let mut grpc_stream = response.into_inner();
    let (mut ws_sender, mut ws_receiver) = socket.split();

    let mut to_grpc = tokio::spawn(async move {
        while let Some(next) = ws_receiver.next().await {
            match next {
                Ok(WsMessage::Binary(bytes)) => {
                    if tx_in
                        .send(ClientMessage {
                            payload: Some(ClientPayload::AudioChunk(AudioChunk {
                                pcm_s16le: bytes.to_vec(),
                                sample_rate: 16_000,
                            })),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(WsMessage::Text(text)) => {
                    if let Some(msg) = ws_text_to_client_message(&text) {
                        if tx_in.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(WsMessage::Close(_)) => break,
                Ok(_) => {}
                Err(err) => {
                    debug!("WebSocket receive error: {err}");
                    break;
                }
            }
        }
    });

    let mut to_ws = tokio::spawn(async move {
        loop {
            match grpc_stream.message().await {
                Ok(Some(msg)) => {
                    let payload = server_message_to_ws_json(msg);
                    if ws_sender
                        .send(WsMessage::Text(payload.to_string()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    let payload =
                        json!({"type": "error", "message": format!("grpc stream error: {err}")});
                    let _ = ws_sender.send(WsMessage::Text(payload.to_string())).await;
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = &mut to_grpc => {
            to_ws.abort();
        }
        _ = &mut to_ws => {
            to_grpc.abort();
        }
    }
}

fn ws_text_to_client_message(text: &str) -> Option<ClientMessage> {
    let value: Value = serde_json::from_str(text).ok()?;
    let msg_type = value.get("type")?.as_str()?;
    match msg_type {
        "set_language_mode" => {
            let mode = match value.get("mode").and_then(Value::as_str).unwrap_or("auto") {
                "manual" => LanguageMode::Manual,
                _ => LanguageMode::Auto,
            };
            let manual = value
                .get("manual_lang")
                .and_then(Value::as_str)
                .and_then(normalize_lang_code)
                .unwrap_or(SourceLanguage::Kazakh);
            Some(ClientMessage {
                payload: Some(ClientPayload::SetLanguageMode(SetLanguageMode {
                    mode: mode as i32,
                    manual_lang: manual as i32,
                })),
            })
        }
        "set_glossary" => {
            let terms = parse_terms_from_ws_value(value.get("terms"));
            Some(ClientMessage {
                payload: Some(ClientPayload::SetGlossary(SetGlossary { terms })),
            })
        }
        "get_settings" => Some(ClientMessage {
            payload: Some(ClientPayload::GetSettings(GetSettings {})),
        }),
        _ => None,
    }
}

fn parse_terms_from_ws_value(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| s.split(&['\n', ',', ';'][..]))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .take(200)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(s)) => s
            .split(&['\n', ',', ';'][..])
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .take(200)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn server_message_to_ws_json(msg: ServerMessage) -> Value {
    match msg.payload {
        Some(ServerPayload::SettingsState(s)) => json!({
            "type": "settings_state",
            "language_mode": mode_to_ui(LanguageMode::try_from(s.language_mode).unwrap_or(LanguageMode::Auto)),
            "manual_source_lang": source_to_ui(SourceLanguage::try_from(s.manual_source_lang).unwrap_or(SourceLanguage::Kazakh)),
            "stt_model_lang": s.stt_model_lang,
            "available_stt_models": s.available_stt_models,
            "custom_glossary_terms": s.custom_glossary_terms,
            "warning": if s.warning.is_empty() { Value::Null } else { Value::String(s.warning) },
        }),
        Some(ServerPayload::Recognized(r)) => json!({
            "type": "recognized",
            "original": r.original,
            "detected_language": source_to_ui(SourceLanguage::try_from(r.detected_language).unwrap_or(SourceLanguage::Kazakh)),
        }),
        Some(ServerPayload::Translated(t)) => json!({
            "type": "translated",
            "original": t.original,
            "detected_language": source_to_ui(SourceLanguage::try_from(t.detected_language).unwrap_or(SourceLanguage::Kazakh)),
            "translations": {"RU": t.ru, "EN": t.en, "KK": t.kk},
        }),
        Some(ServerPayload::Error(e)) => json!({
            "type": "error",
            "message": e.message,
        }),
        None => json!({"type": "error", "message": "empty server payload"}),
    }
}

#[tonic::async_trait]
impl RealtimePipeline for PipelineService {
    type StreamStream =
        Pin<Box<dyn futures_util::Stream<Item = Result<ServerMessage, Status>> + Send + 'static>>;

    async fn stream(
        &self,
        request: Request<Streaming<ClientMessage>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel::<Result<ServerMessage, Status>>(64);
        let svc = self.clone();
        tokio::spawn(async move {
            if let Err(err) = svc.handle_stream(&mut inbound, tx.clone()).await {
                error!("pipeline stream ended with error: {err}");
                let _ = tx
                    .send(Ok(ServerMessage {
                        payload: Some(ServerPayload::Error(ErrorEvent {
                            message: format!("stream error: {err}"),
                        })),
                    }))
                    .await;
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

impl PipelineService {
    async fn handle_stream(
        &self,
        inbound: &mut Streaming<ClientMessage>,
        tx: mpsc::Sender<Result<ServerMessage, Status>>,
    ) -> Result<(), String> {
        let mut session = SessionState::new();
        info!(
            "Started stream session id={} started_ms={}",
            session.session_id, session.session_started_ms
        );
        let warning = if self.cfg.kazakh_stt_engine.eq_ignore_ascii_case("vosk") {
            Some("Vosk mode is not implemented in Rust build; using remote STT.".to_string())
        } else {
            None
        };

        send_settings(&tx, &self.cfg, &session, warning.clone()).await;

        loop {
            let maybe_msg = inbound.message().await.map_err(|e| e.to_string())?;
            let Some(msg) = maybe_msg else {
                break;
            };
            match msg.payload {
                Some(ClientPayload::SetLanguageMode(set)) => {
                    let requested_mode =
                        LanguageMode::try_from(set.mode).unwrap_or(LanguageMode::Auto);
                    session.language_mode = match requested_mode {
                        LanguageMode::Manual => LanguageMode::Manual,
                        _ => LanguageMode::Auto,
                    };
                    let requested_lang =
                        SourceLanguage::try_from(set.manual_lang).unwrap_or(SourceLanguage::Kazakh);
                    if is_supported_lang(requested_lang) {
                        session.manual_source_lang = requested_lang;
                    }
                    send_settings(&tx, &self.cfg, &session, warning.clone()).await;
                }
                Some(ClientPayload::SetGlossary(set)) => {
                    session.custom_glossary_terms = parse_glossary_terms(set.terms);
                    send_settings(&tx, &self.cfg, &session, warning.clone()).await;
                }
                Some(ClientPayload::GetSettings(_)) => {
                    send_settings(&tx, &self.cfg, &session, warning.clone()).await;
                }
                Some(ClientPayload::AudioChunk(chunk)) => {
                    self.handle_audio_chunk(chunk, &tx, &mut session).await;
                }
                None => {}
            }
        }

        if let Some(task) = session.translation_task.take() {
            task.abort();
        }
        info!("Closed stream session id={}", session.session_id);
        Ok(())
    }

    async fn handle_audio_chunk(
        &self,
        chunk: AudioChunk,
        tx: &mpsc::Sender<Result<ServerMessage, Status>>,
        session: &mut SessionState,
    ) {
        if chunk.pcm_s16le.len() < 3200 {
            return;
        }

        let rms = compute_rms(&chunk.pcm_s16le);
        if rms < self.cfg.silence_rms_threshold {
            return;
        }

        let stt_started = Instant::now();
        let active_glossary = merged_glossary_terms(&session.custom_glossary_terms);
        let (mut text, mut detected_lang) = if session.language_mode == LanguageMode::Manual {
            let hint = source_lang_hint(session.manual_source_lang);
            match self
                .transcribe_audio(&chunk.pcm_s16le, hint, &active_glossary)
                .await
            {
                Ok(res) => (res.text, session.manual_source_lang),
                Err(err) => {
                    error!("Transcription error (manual): {err}");
                    return;
                }
            }
        } else {
            match self
                .transcribe_audio(&chunk.pcm_s16le, None, &active_glossary)
                .await
            {
                Ok(res) => {
                    let detected_lang = if let Some(lang) = normalize_lang_code(&res.language_raw) {
                        lang
                    } else if res.language_raw.trim().is_empty()
                        || res.language_raw.eq_ignore_ascii_case("unknown")
                    {
                        let Some(lang) = detect_language_from_text(&res.text) else {
                            info!("Skipped unsupported-script text: '{}'", res.text);
                            return;
                        };
                        lang
                    } else {
                        info!(
                            "Skipped chunk due to unsupported STT language '{}': '{}'",
                            res.language_raw,
                            trim_preview(&res.text, 80)
                        );
                        return;
                    };
                    (res.text, detected_lang)
                }
                Err(err) => {
                    error!("Transcription error (auto): {err}");
                    return;
                }
            }
        };

        let stt_ms = stt_started.elapsed().as_secs_f64() * 1000.0;
        if text.trim().is_empty() {
            return;
        }
        text = normalize_spoken_abbreviations(&text);

        if text.split_whitespace().count() < self.cfg.min_words_to_emit {
            return;
        }

        let now = Instant::now();
        if text == session.last_emitted_text {
            if let Some(last) = session.last_emit_time {
                if now.duration_since(last).as_secs_f64() < self.cfg.repeat_emit_seconds {
                    return;
                }
            }
        }

        let mut incremental = extract_incremental_text(&session.last_emitted_text, &text);
        incremental = normalize_spoken_abbreviations(&incremental);

        if incremental.split_whitespace().count() < self.cfg.min_words_to_emit {
            return;
        }
        if is_low_quality_text(&incremental, &self.cfg) {
            debug!("Filtered low-quality text: '{}'", incremental);
            return;
        }

        let Some(heuristic_lang) = detect_language_from_text(&incremental) else {
            info!("Skipped unsupported-script text: '{}'", incremental);
            return;
        };
        if session.language_mode == LanguageMode::Manual && heuristic_lang != detected_lang {
            info!(
                "Manual lang '{}' overridden by heuristic '{}' for '{}'",
                source_to_ui(detected_lang),
                source_to_ui(heuristic_lang),
                incremental
            );
            detected_lang = heuristic_lang;
        }

        if !is_supported_lang(detected_lang) {
            info!(
                "Skipped unsupported detected language '{}'",
                source_to_ui(detected_lang)
            );
            return;
        }

        session.last_emitted_text = text;
        session.last_emit_time = Some(now);

        if is_hallucination(&incremental, detected_lang) {
            debug!(
                "Filtered hallucination: [{}] '{}'",
                source_to_ui(detected_lang),
                incremental
            );
            return;
        }

        info!(
            "[{}] {} | words={} | rms={:.0} | stt_ms={:.0}",
            source_to_ui(detected_lang),
            incremental,
            incremental.split_whitespace().count(),
            rms,
            stt_ms
        );

        let recognized = ServerMessage {
            payload: Some(ServerPayload::Recognized(RecognizedEvent {
                original: sanitize_text_for_business(&incremental),
                detected_language: detected_lang as i32,
            })),
        };
        let _ = tx.send(Ok(recognized)).await;

        let job_id = session.latest_job_id.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(task) = session.translation_task.take() {
            task.abort();
        }

        let latest_job_id = session.latest_job_id.clone();
        let svc = self.clone();
        let tx_clone = tx.clone();
        let source_text = incremental.clone();
        let source_lang = detected_lang;
        let session_id = session.session_id.clone();
        let session_started_ms = session.session_started_ms;
        let glossary_snapshot = active_glossary.clone();
        session.translation_task = Some(tokio::spawn(async move {
            let started = Instant::now();
            let translations = svc
                .translate_text(&source_text, source_lang, &glossary_snapshot)
                .await;
            let translate_ms = started.elapsed().as_secs_f64() * 1000.0;
            let total_ms = stt_ms + translate_ms;

            if latest_job_id.load(Ordering::SeqCst) != job_id {
                return;
            }

            info!(
                "[pipeline] total_ms={:.0} stt_ms={:.0} translate_ms={:.0} rms={:.0}",
                total_ms, stt_ms, translate_ms, rms
            );
            if let Err(err) = svc
                .history
                .append(
                    source_lang,
                    source_text.clone(),
                    &translations,
                    &session_id,
                    session_started_ms,
                )
                .await
            {
                warn!("Failed to append history entry: {err}");
            }
            let msg = ServerMessage {
                payload: Some(ServerPayload::Translated(TranslatedEvent {
                    original: source_text,
                    detected_language: source_lang as i32,
                    ru: translations.ru,
                    en: translations.en,
                    kk: translations.kk,
                })),
            };
            let _ = tx_clone.send(Ok(msg)).await;
        }));
    }

    async fn transcribe_audio(
        &self,
        pcm_s16le: &[u8],
        language_hint: Option<&str>,
        glossary_terms: &[String],
    ) -> Result<TranscriptionResult, String> {
        let wav = pcm_to_wav_bytes(pcm_s16le, 16_000)?;
        let part = Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| e.to_string())?;

        let mut form = Form::new()
            .text("model", self.cfg.remote_stt_model.clone())
            .text("response_format", "json")
            .part("file", part);
        if let Some(hint) = language_hint {
            form = form.text("language", hint.to_string());
        }
        let glossary_prompt = build_glossary_prompt(glossary_terms);
        if !glossary_prompt.is_empty() {
            form = form.text("prompt", glossary_prompt);
        }

        let response = self
            .http
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.cfg.openai_api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = response.status();
        let body = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("OpenAI STT error {}: {}", status, body));
        }
        let parsed: OpenAiTranscriptionResponse =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;
        Ok(TranscriptionResult {
            text: parsed.text.unwrap_or_default().trim().to_string(),
            language_raw: parsed.language.unwrap_or_else(|| "unknown".to_string()),
        })
    }

    async fn translate_text(
        &self,
        text: &str,
        detected_lang: SourceLanguage,
        glossary_terms: &[String],
    ) -> TranslationTriple {
        let source_name = match detected_lang {
            SourceLanguage::Russian => "Russian",
            SourceLanguage::English => "English",
            _ => "Kazakh",
        };
        let glossary_prompt = build_glossary_prompt(glossary_terms);

        let body = json!({
            "model": self.cfg.translation_model,
            "temperature": 0.2,
            "response_format": {"type": "json_object"},
            "messages": [
                {
                    "role": "system",
                    "content": format!(
                        "You are a real-time conference translator. The source text is in {source_name}. \
        First, rewrite the source into a clean, grammatically correct sentence with natural punctuation, while preserving meaning and without adding facts. \
        Remove filler words and disfluencies (e.g., 'ээ', 'эм', repetitions) unless they change meaning. \
        Translate it accurately into three languages: Russian (RU), English (EN), and Kazakh (KK). \
        For Kazakh, use proper Kazakh Cyrillic script. \
        Strict policy: profanity/obscenity/insults are forbidden in any language. \
        If source contains such words, replace with neutral business-safe wording. \
        {glossary_prompt} Keep each translation concise for subtitle display. \
        Return ONLY valid JSON: {{\"RU\": \"...\", \"EN\": \"...\", \"KK\": \"...\"}}"
                    )
                },
                {"role": "user", "content": format!("Translate: \"{}\"", text)}
            ]
        });

        let response = self
            .http
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.cfg.openai_api_key)
            .json(&body)
            .send()
            .await;

        let Ok(response) = response else {
            error!("Translation request failed");
            let safe = sanitize_text_for_business(text);
            return TranslationTriple {
                ru: safe.clone(),
                en: safe.clone(),
                kk: safe,
            };
        };

        let status = response.status();
        let body_txt = response.text().await.unwrap_or_default();
        if !status.is_success() {
            error!("Translation HTTP error {}: {}", status, body_txt);
            let safe = sanitize_text_for_business(text);
            return TranslationTriple {
                ru: safe.clone(),
                en: safe.clone(),
                kk: safe,
            };
        }

        let content = extract_chat_content(&body_txt).unwrap_or_else(|| "{}".to_string());
        let parsed = serde_json::from_str::<Value>(&content).unwrap_or_else(|_| json!({}));

        let ru = parsed.get("RU").and_then(Value::as_str).unwrap_or(text);
        let en = parsed.get("EN").and_then(Value::as_str).unwrap_or(text);
        let kk = parsed.get("KK").and_then(Value::as_str).unwrap_or(text);

        TranslationTriple {
            ru: sanitize_text_for_business(ru),
            en: sanitize_text_for_business(en),
            kk: sanitize_text_for_business(kk),
        }
    }
}

async fn send_settings(
    tx: &mpsc::Sender<Result<ServerMessage, Status>>,
    cfg: &AppConfig,
    session: &SessionState,
    warning: Option<String>,
) {
    let stt_label = if session.language_mode == LanguageMode::Manual
        && session.manual_source_lang == SourceLanguage::Kazakh
        && cfg.kazakh_stt_engine.eq_ignore_ascii_case("vosk")
    {
        "kazakh-vosk".to_string()
    } else {
        cfg.remote_stt_model.clone()
    };
    let settings = SettingsState {
        language_mode: session.language_mode as i32,
        manual_source_lang: session.manual_source_lang as i32,
        stt_model_lang: stt_label,
        available_stt_models: vec!["kazakh-vosk".to_string(), cfg.remote_stt_model.clone()],
        custom_glossary_terms: session.custom_glossary_terms.clone(),
        warning: warning.unwrap_or_default(),
    };
    let _ = tx
        .send(Ok(ServerMessage {
            payload: Some(ServerPayload::SettingsState(settings)),
        }))
        .await;
}

fn parse_env_usize(key: &str, default: usize) -> Result<usize, String> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<usize>()
            .map_err(|_| format!("{key} must be a number")),
        Err(_) => Ok(default),
    }
}

fn parse_env_f64(key: &str, default: f64) -> Result<f64, String> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<f64>()
            .map_err(|_| format!("{key} must be a number")),
        Err(_) => Ok(default),
    }
}

fn parse_env_addr(key: &str, default: &str) -> Result<SocketAddr, String> {
    let raw = std::env::var(key).unwrap_or_else(|_| default.to_string());
    raw.parse::<SocketAddr>()
        .map_err(|_| format!("{key} must be a socket address, got: {raw}"))
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn compute_rms(pcm_s16le: &[u8]) -> f64 {
    if pcm_s16le.len() < 2 {
        return 0.0;
    }
    let mut sum_sq = 0f64;
    let mut count = 0usize;
    for chunk in pcm_s16le.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f64;
        sum_sq += sample * sample;
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    (sum_sq / count as f64).sqrt()
}

fn pcm_to_wav_bytes(pcm_s16le: &[u8], sample_rate: u32) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(|e| e.to_string())?;
        for chunk in pcm_s16le.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            writer.write_sample(sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}

fn parse_glossary_terms(raw_terms: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in raw_terms {
        for term in raw.split(&['\n', ',', ';'][..]) {
            let cleaned = MULTISPACE_RE.replace_all(term.trim(), " ").to_string();
            if cleaned.is_empty() {
                continue;
            }
            let key = cleaned.to_lowercase();
            if seen.insert(key) {
                out.push(cleaned);
            }
            if out.len() >= 200 {
                return out;
            }
        }
    }
    out
}

fn merged_glossary_terms(custom_terms: &[String]) -> Vec<String> {
    parse_glossary_terms(custom_terms.to_vec())
}

fn build_glossary_prompt(glossary_terms: &[String]) -> String {
    if glossary_terms.is_empty() {
        return String::new();
    }
    let preview = glossary_terms
        .iter()
        .take(60)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Preferred official terms and abbreviations. Use only when acoustically present, do not invent new terms. \
Preserve spelling exactly when they appear or when ASR is close: {preview}"
    )
}

fn normalize_lang_code(raw: &str) -> Option<SourceLanguage> {
    match raw.to_lowercase().as_str() {
        "kk" | "kazakh" => Some(SourceLanguage::Kazakh),
        "ru" | "russian" => Some(SourceLanguage::Russian),
        "en" | "english" => Some(SourceLanguage::English),
        _ => None,
    }
}

fn source_lang_hint(lang: SourceLanguage) -> Option<&'static str> {
    match lang {
        SourceLanguage::Kazakh => Some("kk"),
        SourceLanguage::Russian => Some("ru"),
        SourceLanguage::English => Some("en"),
        SourceLanguage::Unspecified => None,
    }
}

fn is_supported_lang(lang: SourceLanguage) -> bool {
    matches!(
        lang,
        SourceLanguage::Kazakh | SourceLanguage::Russian | SourceLanguage::English
    )
}

fn source_to_ui(lang: SourceLanguage) -> &'static str {
    match lang {
        SourceLanguage::Kazakh => "kazakh",
        SourceLanguage::Russian => "russian",
        SourceLanguage::English => "english",
        SourceLanguage::Unspecified => "unknown",
    }
}

fn mode_to_ui(mode: LanguageMode) -> &'static str {
    match mode {
        LanguageMode::Manual => "manual",
        _ => "auto",
    }
}

fn detect_language_from_text(text: &str) -> Option<SourceLanguage> {
    let lowered = text.to_lowercase();
    if lowered.trim().is_empty() {
        return None;
    }
    if lowered.chars().any(|c| "әіңғүұқөһ".contains(c)) {
        return Some(SourceLanguage::Kazakh);
    }
    let latin_count = lowered.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let cyrillic_count = lowered
        .chars()
        .filter(|c| ('а'..='я').contains(c) || *c == 'ё')
        .count();
    if latin_count > cyrillic_count && latin_count > 0 {
        return Some(SourceLanguage::English);
    }
    if cyrillic_count > 0 {
        return Some(SourceLanguage::Russian);
    }
    None
}

fn is_low_quality_text(text: &str, cfg: &AppConfig) -> bool {
    let value = text.trim();
    if value.is_empty() {
        return true;
    }
    if value.len() < cfg.min_text_length_to_emit {
        return true;
    }
    let alpha_chars = value.chars().filter(|c| c.is_alphabetic()).count();
    if alpha_chars < cfg.min_alpha_chars_to_emit {
        return true;
    }
    let alpha_ratio = alpha_chars as f64 / value.chars().count().max(1) as f64;
    alpha_ratio < cfg.min_alpha_ratio_to_emit
}

fn is_hallucination(text: &str, detected_lang: SourceLanguage) -> bool {
    let cleaned = text
        .trim()
        .to_lowercase()
        .trim_end_matches(['.', '!', '?', ',', ';', ':'])
        .to_string();
    if HALLUCINATION_SET.contains(cleaned.as_str()) {
        return true;
    }
    if cleaned.len() <= 2 {
        return true;
    }
    !is_supported_lang(detected_lang)
}

fn extract_incremental_text(previous: &str, current: &str) -> String {
    let prev_words: Vec<&str> = previous.split_whitespace().collect();
    let curr_words: Vec<&str> = current.split_whitespace().collect();
    if prev_words.is_empty() || curr_words.is_empty() {
        return current.to_string();
    }
    if curr_words.len() <= prev_words.len() {
        return current.to_string();
    }
    if curr_words[..prev_words.len()] == prev_words[..] {
        return curr_words[prev_words.len()..].join(" ");
    }
    current.to_string()
}

fn sanitize_text_for_business(text: &str) -> String {
    let no_prof = PROFANITY_RE.replace_all(text, "").to_string();
    let compact = MULTISPACE_RE.replace_all(no_prof.trim(), " ").to_string();
    SPACE_PUNCT_RE.replace_all(&compact, "$1").to_string()
}

fn normalize_spoken_abbreviations(text: &str) -> String {
    let v = SPOKEN_MNU_RE_1.replace_all(text, "MNU").to_string();
    SPOKEN_MNU_RE_2.replace_all(&v, "MNU").to_string()
}

fn extract_chat_content(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()
        .map(|s| s.to_string())
}

fn trim_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out
}
