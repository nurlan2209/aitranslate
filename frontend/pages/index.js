import { useEffect, useMemo, useRef, useState } from "react";

const BACKEND_URL = process.env.NEXT_PUBLIC_BACKEND_URL || "http://localhost:8090";

const UI_COPY = {
  ru: {
    nav: ["Чат", "Перевод", "История", "Параметры", "Выход"],
    chatTitle: "Чат",
    panelTitle: "Панель управления",
    langTitle: "Выбор языка",
    micTitle: "Аудиозапись",
    connTitle: "Статус подключения",
    connected: "Подключено",
    wsOk: "WebSocket: OK",
    input: "Введите сообщение...",
    send: "Отправить",
    you: "Вы",
    ai: "AI",
    statusNotRecording: "Не записывается",
  },
  kk: {
    nav: ["Чат", "Аудару", "Тарих", "Параметрлер", "Шығу"],
    chatTitle: "Чат",
    panelTitle: "Басқару панелі",
    langTitle: "Тіл таңдау",
    micTitle: "Аудио жазу",
    connTitle: "Қосылу күйі",
    connected: "Қосылған",
    wsOk: "WebSocket: OK",
    input: "Хабарлама жазыңыз...",
    send: "Жіберу",
    you: "Сіз",
    ai: "AI",
    statusNotRecording: "Жазылуда емес",
  },
  en: {
    nav: ["Chat", "Translate", "History", "Settings", "Logout"],
    chatTitle: "Chat",
    panelTitle: "Control panel",
    langTitle: "Language",
    micTitle: "Audio recording",
    connTitle: "Connection",
    connected: "Connected",
    wsOk: "WebSocket: OK",
    input: "Type a message...",
    send: "Send",
    you: "You",
    ai: "AI",
    statusNotRecording: "Not recording",
  },
};

function createDefaultTurnMeta() {
  return { barge_in: false, interrupted_assistant_text: "", interrupted_at_ms: null };
}

function chooseRecorderMimeType() {
  if (typeof MediaRecorder === "undefined") return "";
  const preferred = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4"];
  return preferred.find((type) => MediaRecorder.isTypeSupported(type)) || "";
}

function formatClock(date) {
  return new Intl.DateTimeFormat("ru-RU", { hour: "2-digit", minute: "2-digit" }).format(date);
}

export default function HomePage() {
  const [uiLang, setUiLang] = useState("kk");
  const [messages, setMessages] = useState([]);
  const [inputText, setInputText] = useState("");
  const [isRecording, setIsRecording] = useState(false);
  const [isProcessing, setIsProcessing] = useState(false);

  const recorderRef = useRef(null);
  const playbackRef = useRef(null);
  const streamRef = useRef(null);
  const chunksRef = useRef([]);
  const chatRef = useRef(null);

  const messagesRef = useRef([]);
  const lastAssistantTextRef = useRef("");
  const pendingTurnMetaRef = useRef(createDefaultTurnMeta());

  const t = UI_COPY[uiLang];

  const elapsed = useMemo(() => {
    if (!isRecording) return "00:00:00";
    return "00:00:01";
  }, [isRecording]);

  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  useEffect(() => {
    if (chatRef.current) chatRef.current.scrollTop = chatRef.current.scrollHeight;
  }, [messages]);

  useEffect(() => {
    return () => {
      try {
        recorderRef.current?.stop();
      } catch {}
      playbackRef.current?.pause();
      if (streamRef.current) streamRef.current.getTracks().forEach((track) => track.stop());
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
    } catch {}

    playbackRef.current = null;

    if (markBargeIn) {
      pendingTurnMetaRef.current = {
        barge_in: true,
        interrupted_assistant_text: lastAssistantTextRef.current || "",
        interrupted_at_ms: interruptedAtMs,
      };
    }

    return true;
  };

  const playAssistantAudio = async (audioBase64, audioMimeType) => {
    if (!audioBase64) return;

    stopAssistantPlayback({ markBargeIn: false });
    const audio = new Audio(`data:${audioMimeType || "audio/mpeg"};base64,${audioBase64}`);
    playbackRef.current = audio;

    audio.onended = () => {
      if (playbackRef.current === audio) playbackRef.current = null;
    };

    audio.onerror = () => {
      if (playbackRef.current === audio) playbackRef.current = null;
    };

    try {
      await audio.play();
    } catch {
      if (playbackRef.current === audio) playbackRef.current = null;
    }
  };

  const sendTextToBackend = async () => {
    const text = inputText.trim();
    if (!text || isProcessing || isRecording) return;

    setIsProcessing(true);

    try {
      const interruptedAssistant = stopAssistantPlayback({ markBargeIn: true });
      const turnMeta = pendingTurnMetaRef.current;
      pendingTurnMetaRef.current = createDefaultTurnMeta();

      const response = await fetch(`${BACKEND_URL}/api/chat`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          message: text,
          history: messagesRef.current.slice(-8).map((item) => ({ role: item.role, content: item.content })),
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
      const time = formatClock(new Date());

      setMessages((prev) => [
        ...prev,
        { role: "user", content: text, time },
        { role: "assistant", content: assistantText, time: formatClock(new Date()) },
      ]);

      lastAssistantTextRef.current = assistantText;
      setInputText("");

      if (data.audio_base64) {
        await playAssistantAudio(data.audio_base64, data.audio_mime_type);
      }
    } catch {
      // keep UI minimal for this layout
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
        JSON.stringify(messagesRef.current.slice(-8).map((item) => ({ role: item.role, content: item.content })))
      );
      formData.append("turn_meta", JSON.stringify(turnMeta));

      const response = await fetch(`${BACKEND_URL}/api/voice`, { method: "POST", body: formData });
      if (!response.ok) {
        const err = await response.json().catch(() => ({}));
        throw new Error(err.detail || `HTTP ${response.status}`);
      }

      const data = await response.json();
      const userText = data.transcript || "";
      const assistantText = data.response_text || "";
      const time = formatClock(new Date());

      setMessages((prev) => [
        ...prev,
        { role: "user", content: userText, time },
        { role: "assistant", content: assistantText, time: formatClock(new Date()) },
      ]);
      lastAssistantTextRef.current = assistantText;

      await playAssistantAudio(data.audio_base64, data.audio_mime_type);
    } catch {
      // keep UI minimal for this layout
    } finally {
      setIsProcessing(false);
      stopStream();
    }
  };

  const startRecording = async () => {
    if (isRecording || isProcessing) return;

    try {
      stopAssistantPlayback({ markBargeIn: true });
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      streamRef.current = stream;
      chunksRef.current = [];

      const mimeType = chooseRecorderMimeType();
      const mediaRecorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);

      mediaRecorder.ondataavailable = (event) => {
        if (event.data && event.data.size > 0) chunksRef.current.push(event.data);
      };

      mediaRecorder.onstop = async () => {
        const blobType = mediaRecorder.mimeType || "audio/webm";
        const audioBlob = new Blob(chunksRef.current, { type: blobType });
        await sendAudioToBackend(audioBlob);
      };

      recorderRef.current = mediaRecorder;
      mediaRecorder.start();
      setIsRecording(true);
    } catch {
      // ignore
    }
  };

  const stopRecording = () => {
    if (!isRecording) return;
    setIsRecording(false);
    setIsProcessing(true);
    recorderRef.current?.stop();
    recorderRef.current = null;
  };

  return (
    <main className="appShell">
      <aside className="leftNav">
        <div className="logoMark">◼</div>
        {t.nav.map((item, idx) => (
          <button key={item} className={`navItem ${idx === 0 ? "active" : ""}`} type="button">
            {item}
          </button>
        ))}
      </aside>

      <section className="centerArea">
        <div className="board">
          <div className="chatCol">
            <h2>{t.chatTitle}</h2>
            <div className="messages" ref={chatRef}>
              {messages.map((message, index) => (
                <article key={`${message.role}-${index}`} className={`msgCard ${message.role}`}>
                  <div className="msgHead">
                    <span className="avatar">{message.role === "assistant" ? "AI" : "A"}</span>
                    <strong>{message.role === "assistant" ? t.ai : t.you}</strong>
                    <span className="time">{message.time}</span>
                  </div>
                  <p>{message.content}</p>
                </article>
              ))}
            </div>
          </div>

          <aside className="rightPanel">
            <h3>{t.panelTitle}</h3>

            <div className="panelCard">
              <h4>{t.langTitle}</h4>
              <select value={uiLang} onChange={(e) => setUiLang(e.target.value)}>
                <option value="kk">Қазақша → English</option>
                <option value="ru">Русский → English</option>
                <option value="en">English → Қазақша</option>
              </select>
            </div>

            <div className="panelCard">
              <h4>{t.micTitle}</h4>
              <button
                type="button"
                className={`micBtn ${isRecording ? "recording" : ""}`}
                onClick={isRecording ? stopRecording : startRecording}
                disabled={isProcessing}
              >
                🎤
              </button>
              <div className="timer">{elapsed}</div>
              <div className="sub">{t.statusNotRecording}</div>
            </div>

            <div className="panelCard">
              <h4>{t.connTitle}</h4>
              <p className="okDot">● {t.connected}</p>
              <p className="wsText">{t.wsOk}</p>
            </div>
          </aside>
        </div>

        <form
          className="composer"
          onSubmit={(event) => {
            event.preventDefault();
            sendTextToBackend();
          }}
        >
          <input
            value={inputText}
            onChange={(event) => setInputText(event.target.value)}
            placeholder={t.input}
            disabled={isProcessing || isRecording}
          />
          <div className="actions">
            <button type="button" className="iconBtn" onClick={isRecording ? stopRecording : startRecording}>
              🎙
            </button>
            <button type="submit" className="sendBtn" disabled={!inputText.trim() || isProcessing || isRecording}>
              {t.send}
            </button>
          </div>
        </form>
      </section>
    </main>
  );
}
