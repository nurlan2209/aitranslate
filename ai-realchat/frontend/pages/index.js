import { useEffect, useMemo, useRef, useState } from "react";

const BACKEND_URL = process.env.NEXT_PUBLIC_BACKEND_URL || "http://localhost:8090";

const UI_COPY = {
  ru: {
    title: "AI RealChat",
    subtitle: "Режимы как в ChatGPT: обычный чат и realtime-голос.",
    modeLabel: "Режим",
    modeChat: "Чат",
    modeVoice: "Realtime голос",
    start: "Начать говорить",
    stop: "Остановить",
    clear: "Очистить диалог",
    send: "Отправить",
    textPlaceholder: "Напишите сообщение...",
    languageLabel: "Язык интерфейса",
    you: "Вы",
    assistant: "Ассистент",
    empty: "Диалог пока пуст. Напишите сообщение или начните запись.",
    status: {
      idle: "Готов",
      listening: "Слушаю...",
      processing: "Обрабатываю...",
      speaking: "Озвучиваю ответ...",
      interrupted: "Озвучка прервана, слушаю вас",
      ready: "Ответ готов",
      mic_error: "Не удалось получить доступ к микрофону",
      request_error: "Ошибка запроса к серверу",
      play_error: "Не удалось автоматически воспроизвести аудио",
    },
  },
  kk: {
    title: "AI RealChat",
    subtitle: "ChatGPT сияқты режимдер: кәдімгі чат және realtime дауыс.",
    modeLabel: "Режим",
    modeChat: "Чат",
    modeVoice: "Realtime дауыс",
    start: "Сөйлесуді бастау",
    stop: "Тоқтату",
    clear: "Диалогты тазалау",
    send: "Жіберу",
    textPlaceholder: "Хабарлама жазыңыз...",
    languageLabel: "Интерфейс тілі",
    you: "Сіз",
    assistant: "Ассистент",
    empty: "Диалог әлі бос. Хабарлама жазыңыз немесе жазуды бастаңыз.",
    status: {
      idle: "Дайын",
      listening: "Тыңдап тұрмын...",
      processing: "Өңдеп жатырмын...",
      speaking: "Жауапты дыбыстап жатырмын...",
      interrupted: "Дыбыс тоқтатылды, сізді тыңдап тұрмын",
      ready: "Жауап дайын",
      mic_error: "Микрофонға қол жеткізу мүмкін болмады",
      request_error: "Серверге сұрау қатесі",
      play_error: "Аудионы автоматты ойнату мүмкін болмады",
    },
  },
  en: {
    title: "AI RealChat",
    subtitle: "ChatGPT-like modes: standard chat and realtime voice.",
    modeLabel: "Mode",
    modeChat: "Chat",
    modeVoice: "Realtime voice",
    start: "Start speaking",
    stop: "Stop",
    clear: "Clear chat",
    send: "Send",
    textPlaceholder: "Type your message...",
    languageLabel: "UI language",
    you: "You",
    assistant: "Assistant",
    empty: "No messages yet. Type a message or start recording.",
    status: {
      idle: "Ready",
      listening: "Listening...",
      processing: "Processing...",
      speaking: "Speaking response...",
      interrupted: "Playback interrupted, listening to you",
      ready: "Response is ready",
      mic_error: "Could not access microphone",
      request_error: "Request to server failed",
      play_error: "Audio autoplay was blocked",
    },
  },
};

const LANG_OPTIONS = [
  { code: "ru", label: "RU" },
  { code: "kk", label: "KZ" },
  { code: "en", label: "EN" },
];

const MODE_OPTIONS = [
  { code: "chat" },
  { code: "voice" },
];

function createDefaultTurnMeta() {
  return {
    barge_in: false,
    interrupted_assistant_text: "",
    interrupted_at_ms: null,
  };
}

function chooseRecorderMimeType() {
  if (typeof MediaRecorder === "undefined") return "";
  const preferred = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4"];
  return preferred.find((type) => MediaRecorder.isTypeSupported(type)) || "";
}

export default function HomePage() {
  const [uiLang, setUiLang] = useState("ru");
  const [interactionMode, setInteractionMode] = useState("chat");
  const [messages, setMessages] = useState([]);
  const [inputText, setInputText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);
  const [assistantSpeaking, setAssistantSpeaking] = useState(false);
  const [statusKey, setStatusKey] = useState("idle");
  const [statusDetail, setStatusDetail] = useState("");

  const recorderRef = useRef(null);
  const playbackRef = useRef(null);
  const streamRef = useRef(null);
  const chunksRef = useRef([]);
  const chatRef = useRef(null);

  const messagesRef = useRef([]);
  const lastAssistantTextRef = useRef("");
  const pendingTurnMetaRef = useRef(createDefaultTurnMeta());

  const t = UI_COPY[uiLang];

  const statusText = useMemo(() => {
    const base = t.status[statusKey] || t.status.idle;
    return statusDetail ? `${base}: ${statusDetail}` : base;
  }, [t, statusKey, statusDetail]);

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  useEffect(() => {
    const chat = chatRef.current;
    if (!chat) return;
    chat.scrollTop = chat.scrollHeight;
  }, [messages]);

  useEffect(() => {
    return () => {
      try {
        recorderRef.current?.stop();
      } catch {
        // Ignore recorder shutdown errors on unmount.
      }

      const activeAudio = playbackRef.current;
      if (activeAudio) {
        activeAudio.pause();
      }

      if (streamRef.current) {
        streamRef.current.getTracks().forEach((track) => track.stop());
      }
    };
  }, []);

  const stopStream = () => {
    if (!streamRef.current) return;
    streamRef.current.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
  };

  const stopAssistantPlayback = ({ markBargeIn }) => {
    const activeAudio = playbackRef.current;
    if (!activeAudio) return false;

    const interruptedAtMs = Math.max(0, Math.floor((activeAudio.currentTime || 0) * 1000));
    activeAudio.pause();
    try {
      activeAudio.currentTime = 0;
    } catch {
      // Ignore seek errors for short/streamed blobs.
    }

    playbackRef.current = null;
    setAssistantSpeaking(false);

    if (markBargeIn) {
      pendingTurnMetaRef.current = {
        barge_in: true,
        interrupted_assistant_text: lastAssistantTextRef.current || "",
        interrupted_at_ms: interruptedAtMs,
      };
      setStatusDetail("");
      setStatusKey("interrupted");
    }

    return true;
  };

  const playAssistantAudio = async (audioBase64, audioMimeType) => {
    if (!audioBase64) return;

    stopAssistantPlayback({ markBargeIn: false });

    const audio = new Audio(`data:${audioMimeType || "audio/mpeg"};base64,${audioBase64}`);
    playbackRef.current = audio;
    setAssistantSpeaking(true);

    audio.onended = () => {
      if (playbackRef.current === audio) {
        playbackRef.current = null;
      }
      setAssistantSpeaking(false);
      setStatusKey("ready");
    };

    audio.onerror = () => {
      if (playbackRef.current === audio) {
        playbackRef.current = null;
      }
      setAssistantSpeaking(false);
    };

    try {
      await audio.play();
      setStatusKey("speaking");
    } catch (playError) {
      if (playbackRef.current === audio) {
        playbackRef.current = null;
      }
      setAssistantSpeaking(false);
      setStatusKey("play_error");
      setStatusDetail(playError instanceof Error ? playError.message : "");
    }
  };

  const sendTextToBackend = async () => {
    const text = inputText.trim();
    if (!text || isProcessing || isRecording) return;

    setIsProcessing(true);
    setStatusDetail("");
    setStatusKey("processing");

    try {
      const interruptedAssistant = stopAssistantPlayback({ markBargeIn: true });
      const turnMeta = pendingTurnMetaRef.current;
      pendingTurnMetaRef.current = createDefaultTurnMeta();

      const response = await fetch(`${BACKEND_URL}/api/chat`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          message: text,
          history: messagesRef.current.slice(-8).map((item) => ({
            role: item.role,
            content: item.content,
          })),
          turn_meta: interruptedAssistant ? turnMeta : createDefaultTurnMeta(),
          speak: false,
        }),
      });

      if (!response.ok) {
        const err = await response.json().catch(() => ({}));
        throw new Error(err.detail || `HTTP ${response.status}`);
      }

      const data = await response.json();
      const assistantText = data.response_text || "";

      setMessages((prev) => [
        ...prev,
        { role: "user", content: text },
        { role: "assistant", content: assistantText },
      ]);
      lastAssistantTextRef.current = assistantText;
      setInputText("");
      setStatusDetail("");

      if (data.audio_base64) {
        await playAssistantAudio(data.audio_base64, data.audio_mime_type);
      } else {
        setStatusKey("ready");
      }
    } catch (error) {
      setStatusKey("request_error");
      setStatusDetail(error instanceof Error ? error.message : "");
    } finally {
      setIsProcessing(false);
    }
  };

  const sendAudioToBackend = async (audioBlob) => {
    try {
      const formData = new FormData();
      const extension = audioBlob.type.includes("mp4") ? "m4a" : "webm";
      const turnMeta = pendingTurnMetaRef.current;
      pendingTurnMetaRef.current = createDefaultTurnMeta();

      formData.append("file", audioBlob, `speech.${extension}`);
      formData.append(
        "history",
        JSON.stringify(
          messagesRef.current.slice(-8).map((item) => ({
            role: item.role,
            content: item.content,
          }))
        )
      );
      formData.append("turn_meta", JSON.stringify(turnMeta));

      const response = await fetch(`${BACKEND_URL}/api/voice`, {
        method: "POST",
        body: formData,
      });

      if (!response.ok) {
        const err = await response.json().catch(() => ({}));
        throw new Error(err.detail || `HTTP ${response.status}`);
      }

      const data = await response.json();
      const userText = data.transcript || "";
      const assistantText = data.response_text || "";

      setMessages((prev) => [
        ...prev,
        { role: "user", content: userText },
        { role: "assistant", content: assistantText },
      ]);
      lastAssistantTextRef.current = assistantText;
      setStatusDetail("");

      await playAssistantAudio(data.audio_base64, data.audio_mime_type);
      if (!data.audio_base64) {
        setStatusKey("ready");
      }
    } catch (error) {
      setStatusKey("request_error");
      setStatusDetail(error instanceof Error ? error.message : "");
    } finally {
      setIsProcessing(false);
      stopStream();
    }
  };

  const startRecording = async () => {
    if (isRecording || isProcessing) return;

    let interruptedAssistant = false;
    try {
      interruptedAssistant = stopAssistantPlayback({ markBargeIn: true });

      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      streamRef.current = stream;
      chunksRef.current = [];

      const mimeType = chooseRecorderMimeType();
      const mediaRecorder = mimeType
        ? new MediaRecorder(stream, { mimeType })
        : new MediaRecorder(stream);

      mediaRecorder.ondataavailable = (event) => {
        if (event.data && event.data.size > 0) {
          chunksRef.current.push(event.data);
        }
      };

      mediaRecorder.onstop = async () => {
        const blobType = mediaRecorder.mimeType || "audio/webm";
        const audioBlob = new Blob(chunksRef.current, { type: blobType });
        await sendAudioToBackend(audioBlob);
      };

      recorderRef.current = mediaRecorder;
      mediaRecorder.start();

      setIsRecording(true);
      setStatusDetail("");
      setStatusKey("listening");
    } catch (error) {
      if (interruptedAssistant) {
        pendingTurnMetaRef.current = createDefaultTurnMeta();
      }
      setStatusKey("mic_error");
      setStatusDetail(error instanceof Error ? error.message : "");
    }
  };

  const stopRecording = () => {
    if (!isRecording) return;

    setIsRecording(false);
    setIsProcessing(true);
    setStatusDetail("");
    setStatusKey("processing");

    recorderRef.current?.stop();
    recorderRef.current = null;
  };

  const switchInteractionMode = (mode) => {
    if (mode === interactionMode) return;
    if (isRecording || isProcessing) return;

    stopAssistantPlayback({ markBargeIn: false });
    setInteractionMode(mode);
    setStatusKey("idle");
    setStatusDetail("");
  };

  const clearChat = () => {
    if (isRecording || isProcessing) return;
    stopAssistantPlayback({ markBargeIn: false });
    pendingTurnMetaRef.current = createDefaultTurnMeta();
    lastAssistantTextRef.current = "";
    setMessages([]);
    setInputText("");
    setStatusKey("idle");
    setStatusDetail("");
  };

  const isBusy = isProcessing || isRecording;

  return (
    <main className="page">
      <div className="noise" />
      <section className="card">
        <header className="cardHead">
          <div>
            <h1>{t.title}</h1>
            <p>{t.subtitle}</p>
          </div>

          <div className="languageBox" aria-label={t.languageLabel}>
            <span>{t.languageLabel}</span>
            <div className="chips">
              {LANG_OPTIONS.map((lang) => (
                <button
                  type="button"
                  key={lang.code}
                  className={`chip ${uiLang === lang.code ? "active" : ""}`}
                  onClick={() => setUiLang(lang.code)}
                >
                  {lang.label}
                </button>
              ))}
            </div>
          </div>
        </header>

        <section className="modeSwitch" aria-label={t.modeLabel}>
          {MODE_OPTIONS.map((mode) => {
            const isActive = interactionMode === mode.code;
            const label = mode.code === "chat" ? t.modeChat : t.modeVoice;
            return (
              <button
                key={mode.code}
                type="button"
                className={`modeBtn ${isActive ? "active" : ""}`}
                onClick={() => switchInteractionMode(mode.code)}
                disabled={isBusy}
              >
                {label}
              </button>
            );
          })}
        </section>

        {interactionMode === "chat" ? (
          <section className="chatComposerWrap">
            <form
              className="chatComposer"
              onSubmit={(event) => {
                event.preventDefault();
                sendTextToBackend();
              }}
            >
              <input
                className="textInput"
                type="text"
                value={inputText}
                onChange={(event) => setInputText(event.target.value)}
                placeholder={t.textPlaceholder}
                disabled={isBusy}
              />
              <button type="submit" className="sendBtn" disabled={isBusy || !inputText.trim()}>
                {t.send}
              </button>
            </form>

            <button type="button" className="clearBtn" onClick={clearChat} disabled={isBusy}>
              {t.clear}
            </button>
          </section>
        ) : (
          <section className="controls">
            <button
              type="button"
              className={`recordBtn ${isRecording ? "recording" : ""}`}
              onClick={isRecording ? stopRecording : startRecording}
              disabled={isProcessing}
            >
              <span className="dot" />
              {isRecording ? t.stop : t.start}
            </button>

            <button type="button" className="clearBtn" onClick={clearChat} disabled={isBusy}>
              {t.clear}
            </button>
          </section>
        )}

        {assistantSpeaking && <div className="speakingBadge">{t.status.speaking}</div>}

        <p className={`status status-${statusKey}`}>{statusText}</p>

        <section ref={chatRef} className="chat" aria-live="polite">
          {messages.length === 0 && <div className="empty">{t.empty}</div>}

          {messages.map((message, index) => (
            <article
              key={`${message.role}-${index}`}
              className={`bubble ${message.role === "assistant" ? "assistant" : "user"}`}
            >
              <span className="roleLabel">
                {message.role === "assistant" ? t.assistant : t.you}
              </span>
              <p>{message.content}</p>
            </article>
          ))}
        </section>
      </section>
    </main>
  );
}
