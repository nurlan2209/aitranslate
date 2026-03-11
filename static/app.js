const SAMPLE_RATE = 16000;
const CHUNK_SECONDS = 6;
const MIN_CHUNK_SECONDS = 1.2;
const SILENCE_CLOSE_SECONDS = 1.0;
const VAD_RMS_THRESHOLD = 450;
const MIC_STORAGE_KEY = "overlay_selected_mic_id";
const LANG_MODE_STORAGE_KEY = "overlay_lang_mode";
const MANUAL_LANG_STORAGE_KEY = "overlay_manual_lang";
const GLOSSARY_STORAGE_KEY = "overlay_glossary_terms";

const els = {
  startBtn: document.getElementById("start-btn"),
  clearBtn: document.getElementById("clear-btn"),
  status: document.getElementById("status"),
  settingsBtn: document.getElementById("settings-btn"),
  closeSettings: document.getElementById("close-settings"),
  settingsPanel: document.getElementById("settings-panel"),
  settingsBackdrop: document.getElementById("settings-backdrop"),
  lineTop: document.getElementById("lineTop"),
  lineBottom: document.getElementById("lineBottom"),
  original: document.getElementById("original"),
  tagTop: document.getElementById("tagTop"),
  tagBottom: document.getElementById("tagBottom"),
  micInput: document.getElementById("micInput"),
  refreshMics: document.getElementById("refreshMics"),
  langMode: document.getElementById("langMode"),
  manualLang: document.getElementById("manualLang"),
  fontSize: document.getElementById("fontSize"),
  fontSizeVal: document.getElementById("fontSizeVal"),
  bottomOffset: document.getElementById("bottomOffset"),
  bottomOffsetVal: document.getElementById("bottomOffsetVal"),
  sideMargin: document.getElementById("sideMargin"),
  sideMarginVal: document.getElementById("sideMarginVal"),
  lineHeight: document.getElementById("lineHeight"),
  lineHeightVal: document.getElementById("lineHeightVal"),
  textAlign: document.getElementById("textAlign"),
  charScale: document.getElementById("charScale"),
  charScaleVal: document.getElementById("charScaleVal"),
  glossaryTerms: document.getElementById("glossaryTerms"),
  saveGlossary: document.getElementById("saveGlossary"),
};

const state = {
  fontSize: 52,
  panelOpacity: 72,
  panelBlur: 9,
  bottomOffset: 14,
  sideMargin: 18,
  lineHeight: 1.14,
  textAlign: "center",
  charScale: 100,
  showStatus: true,
  flashUpdates: true,
  languageMode: "manual",
  manualLang: "kazakh",
  glossaryTerms: [],
  selectedMicId: "",
  missingMicAlertShown: false,
};

let socket = null;
let audioContext = null;
let mediaStream = null;
let processorNode = null;
let pcmBuffer = [];
let silenceSamples = 0;
let isRecording = false;

const lineBuffers = { top: "", bottom: "" };
let currentLayoutKey = "RU|EN";

function setStatus(text) {
  els.status.textContent = text;
}

function pulse(...nodes) {
  if (!state.flashUpdates) return;
  nodes.forEach((n) => {
    n.classList.add("flash");
    setTimeout(() => n.classList.remove("flash"), 120);
  });
}

function applyBackgroundMode(mode) {
  document.documentElement.style.setProperty("--overlay-bg", "#0b1220");
}

function estimateMaxChars(node) {
  const width = Math.max(180, node.clientWidth || window.innerWidth * 0.8);
  const avgCharPx = Math.max(8, state.fontSize * 0.58 * (state.charScale / 100));
  return Math.max(36, Math.floor(width / avgCharPx) * 2);
}

function normalizeSpaces(text) {
  return String(text || "").replace(/\s+/g, " ").trim();
}

function appendTrimmed(current, incoming, maxChars) {
  const base = normalizeSpaces(current);
  const add = normalizeSpaces(incoming);
  if (!add) return base;

  let joined = base ? `${base} ${add}` : add;
  if (joined.length <= maxChars) return joined;

  const words = joined.split(" ");
  while (words.length > 1 && words.join(" ").length > maxChars) {
    words.shift();
  }
  joined = words.join(" ");
  return joined.length > maxChars ? joined.slice(-maxChars) : joined;
}

function getTranslation(t, code) {
  if (!t || !code) return "";
  return t[code] || t[code.toUpperCase()] || t[code.toLowerCase()] || "";
}

function pushLine(which, text) {
  const node = which === "top" ? els.lineTop : els.lineBottom;
  const maxChars = estimateMaxChars(node);
  lineBuffers[which] = appendTrimmed(lineBuffers[which], text, maxChars);
  node.textContent = lineBuffers[which];
}

function reflowBuffers() {
  const maxTop = estimateMaxChars(els.lineTop);
  const maxBottom = estimateMaxChars(els.lineBottom);
  lineBuffers.top = appendTrimmed("", lineBuffers.top, maxTop);
  lineBuffers.bottom = appendTrimmed("", lineBuffers.bottom, maxBottom);
  els.lineTop.textContent = lineBuffers.top;
  els.lineBottom.textContent = lineBuffers.bottom;
}

function syncValueLabels() {
  els.fontSizeVal.textContent = `${state.fontSize}px`;
  els.bottomOffsetVal.textContent = `${state.bottomOffset}px`;
  els.sideMarginVal.textContent = `${state.sideMargin}px`;
  els.lineHeightVal.textContent = `${state.lineHeight.toFixed(2)}`;
  els.charScaleVal.textContent = `${state.charScale}%`;
}

function applySettings() {
  document.documentElement.style.setProperty("--font-size", `${state.fontSize}px`);
  document.documentElement.style.setProperty("--panel-blur", `${state.panelBlur}px`);
  document.documentElement.style.setProperty("--bottom-offset", `${state.bottomOffset}px`);
  document.documentElement.style.setProperty("--side-margin", `${state.sideMargin}px`);
  document.documentElement.style.setProperty("--line-height", `${state.lineHeight}`);
  document.documentElement.style.setProperty("--text-align", state.textAlign);
  els.status.style.display = state.showStatus ? "block" : "none";
  applyBackgroundMode();
  syncValueLabels();
  reflowBuffers();
}

function openSettings() {
  els.settingsPanel.classList.add("open");
  els.settingsBackdrop.classList.add("open");
  refreshMicrophoneList();
}

function closeSettings() {
  els.settingsPanel.classList.remove("open");
  els.settingsBackdrop.classList.remove("open");
}

function connectWebSocket() {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  socket = new WebSocket(`${protocol}//${window.location.host}/ws/audio`);
  socket.binaryType = "arraybuffer";

  socket.onopen = () => {
    setStatus("подключено");
    sendLanguageMode();
    sendGlossaryTerms();
  };
  socket.onclose = () => {
    if (isRecording) {
      setStatus("переподключение");
      setTimeout(connectWebSocket, 1200);
    } else {
      setStatus("отключено");
    }
  };
  socket.onerror = () => setStatus("ошибка");

  socket.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data);
      if (data.type === "settings_state") {
        if (data.language_mode) state.languageMode = data.language_mode;
        if (data.manual_source_lang) state.manualLang = data.manual_source_lang;
        if (Array.isArray(data.custom_glossary_terms)) {
          state.glossaryTerms = data.custom_glossary_terms.map((x) => String(x || "").trim()).filter(Boolean);
          els.glossaryTerms.value = state.glossaryTerms.join("\n");
          localStorage.setItem(GLOSSARY_STORAGE_KEY, els.glossaryTerms.value);
        }
        els.langMode.value = state.languageMode;
        els.manualLang.value = state.manualLang;
        els.manualLang.disabled = state.languageMode !== "manual";
        if (data.warning) {
          setStatus(data.warning);
        } else if (data.stt_model_lang) {
          setStatus(`STT модель: ${data.stt_model_lang}`);
        }
        return;
      }
      if (data.type === "recognized") {
        els.original.textContent = data.original || "";
        return;
      }
      if (data.type !== "translated") return;

      const t = data.translations || {};
      const src = String(data.detected_language || "").toLowerCase();
      let topLang = "RU";
      let bottomLang = "EN";
      let topText = getTranslation(t, "RU");
      let bottomText = getTranslation(t, "EN");

      if (src === "russian") {
        topLang = "KK";
        bottomLang = "EN";
        topText = getTranslation(t, "KK");
        bottomText = getTranslation(t, "EN");
      } else if (src === "english") {
        topLang = "KK";
        bottomLang = "RU";
        topText = getTranslation(t, "KK");
        bottomText = getTranslation(t, "RU");
      }

      const nextLayoutKey = `${topLang}|${bottomLang}`;
      if (nextLayoutKey !== currentLayoutKey) {
        currentLayoutKey = nextLayoutKey;
        clearSubtitleLinesOnly();
      }

      els.tagTop.textContent = topLang;
      els.tagBottom.textContent = bottomLang;
      pushLine("top", topText);
      pushLine("bottom", bottomText);
      els.original.textContent = data.original || "";
      pulse(els.lineTop, els.lineBottom);
    } catch {
      // no-op
    }
  };
}

function sendAudioChunk() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  if (pcmBuffer.length === 0) return;

  const int16Array = new Int16Array(pcmBuffer.length);
  for (let i = 0; i < pcmBuffer.length; i++) {
    int16Array[i] = Math.round(pcmBuffer[i]);
  }
  socket.send(int16Array.buffer);
  pcmBuffer = [];
  silenceSamples = 0;
}

function sendLanguageMode() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  socket.send(JSON.stringify({
    type: "set_language_mode",
    mode: state.languageMode,
    manual_lang: state.manualLang,
  }));
}

function parseGlossaryTermsFromText(raw) {
  return String(raw || "")
    .split(/\r?\n|,|;/)
    .map((item) => item.trim())
    .filter(Boolean)
    .slice(0, 200);
}

function sendGlossaryTerms() {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  socket.send(JSON.stringify({
    type: "set_glossary",
    terms: state.glossaryTerms,
  }));
}

function clearOverlay() {
  lineBuffers.top = "";
  lineBuffers.bottom = "";
  els.lineTop.textContent = "";
  els.lineBottom.textContent = "";
  els.original.textContent = "";
}

function clearSubtitleLinesOnly() {
  lineBuffers.top = "";
  lineBuffers.bottom = "";
  els.lineTop.textContent = "";
  els.lineBottom.textContent = "";
}

async function refreshMicrophoneList() {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const inputs = devices.filter((d) => d.kind === "audioinput");
    const storedMicId = localStorage.getItem(MIC_STORAGE_KEY) || "";
    const previousId = state.selectedMicId || storedMicId;

    els.micInput.innerHTML = "";
    const defaultOption = document.createElement("option");
    defaultOption.value = "";
    defaultOption.textContent = "Системный по умолчанию";
    els.micInput.appendChild(defaultOption);

    for (const input of inputs) {
      const option = document.createElement("option");
      option.value = input.deviceId;
      option.textContent = input.label || `Микрофон ${els.micInput.length}`;
      els.micInput.appendChild(option);
    }

    if (previousId && inputs.some((i) => i.deviceId === previousId)) {
      els.micInput.value = previousId;
      state.selectedMicId = previousId;
      localStorage.setItem(MIC_STORAGE_KEY, previousId);
      state.missingMicAlertShown = false;
    } else {
      els.micInput.value = "";
      state.selectedMicId = "";
      localStorage.removeItem(MIC_STORAGE_KEY);
      if (previousId && !state.missingMicAlertShown) {
        state.missingMicAlertShown = true;
        alert("Ранее выбранный микрофон не найден. Выберите микрофон в настройках.");
      }
    }
  } catch {
    // no-op
  }
}

async function ensureMicrophoneLabels() {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach((t) => t.stop());
  } catch {
    // no-op
  }
  await refreshMicrophoneList();
}

async function startRecording() {
  try {
    const constraints = {
      audio: {
        channelCount: 1,
        sampleRate: SAMPLE_RATE,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
        ...(state.selectedMicId ? { deviceId: { exact: state.selectedMicId } } : {}),
      },
    };

    try {
      mediaStream = await navigator.mediaDevices.getUserMedia(constraints);
    } catch {
      mediaStream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          sampleRate: SAMPLE_RATE,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      state.selectedMicId = "";
      els.micInput.value = "";
    }

    audioContext = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: SAMPLE_RATE });
    const source = audioContext.createMediaStreamSource(mediaStream);
    processorNode = audioContext.createScriptProcessor(4096, 1, 1);
    pcmBuffer = [];
    silenceSamples = 0;

    processorNode.onaudioprocess = (event) => {
      if (!isRecording) return;
      const inputData = event.inputBuffer.getChannelData(0);
      const framePcm = new Array(inputData.length);
      let frameEnergy = 0;
      for (let i = 0; i < inputData.length; i++) {
        const s = Math.max(-1, Math.min(1, inputData[i]));
        const pcm = s < 0 ? s * 0x8000 : s * 0x7fff;
        framePcm[i] = pcm;
        frameEnergy += pcm * pcm;
      }

      const frameRms = Math.sqrt(frameEnergy / Math.max(1, framePcm.length));
      const isSpeechFrame = frameRms >= VAD_RMS_THRESHOLD;

      // Do not accumulate leading silence before speech starts.
      if (!pcmBuffer.length && !isSpeechFrame) {
        return;
      }

      for (let i = 0; i < framePcm.length; i++) {
        pcmBuffer.push(framePcm[i]);
      }

      if (isSpeechFrame) {
        silenceSamples = 0;
      } else {
        silenceSamples += framePcm.length;
      }

      const maxChunkSamples = Math.floor(SAMPLE_RATE * CHUNK_SECONDS);
      const minChunkSamples = Math.floor(SAMPLE_RATE * MIN_CHUNK_SECONDS);
      const silenceCloseSamples = Math.floor(SAMPLE_RATE * SILENCE_CLOSE_SECONDS);

      if (pcmBuffer.length >= maxChunkSamples) {
        sendAudioChunk();
        return;
      }
      if (pcmBuffer.length >= minChunkSamples && silenceSamples >= silenceCloseSamples) {
        sendAudioChunk();
      }
    };

    source.connect(processorNode);
    processorNode.connect(audioContext.destination);
    isRecording = true;
    connectWebSocket();
    els.startBtn.textContent = "⏹ Stop";
    await refreshMicrophoneList();
  } catch {
    alert("Не удалось получить доступ к микрофону.");
  }
}

function stopRecording() {
  isRecording = false;
  sendAudioChunk();

  if (processorNode) {
    processorNode.disconnect();
    processorNode = null;
  }
  if (audioContext) {
    audioContext.close();
    audioContext = null;
  }
  if (mediaStream) {
    mediaStream.getTracks().forEach((t) => t.stop());
    mediaStream = null;
  }
  if (socket) {
    socket.close();
    socket = null;
  }
  els.startBtn.textContent = "🎙 Start";
  setStatus("отключено");
}

els.startBtn.addEventListener("click", () => {
  if (isRecording) stopRecording();
  else startRecording();
});

els.settingsBtn.addEventListener("click", openSettings);
els.closeSettings.addEventListener("click", closeSettings);
els.settingsBackdrop.addEventListener("click", closeSettings);
els.clearBtn.addEventListener("click", clearOverlay);

els.micInput.addEventListener("change", () => {
  state.selectedMicId = els.micInput.value;
  if (state.selectedMicId) {
    localStorage.setItem(MIC_STORAGE_KEY, state.selectedMicId);
  } else {
    localStorage.removeItem(MIC_STORAGE_KEY);
  }
});
els.refreshMics.addEventListener("click", ensureMicrophoneLabels);
els.langMode.addEventListener("change", () => {
  state.languageMode = els.langMode.value;
  els.manualLang.disabled = state.languageMode !== "manual";
  localStorage.setItem(LANG_MODE_STORAGE_KEY, state.languageMode);
  sendLanguageMode();
});
els.manualLang.addEventListener("change", () => {
  state.manualLang = els.manualLang.value;
  localStorage.setItem(MANUAL_LANG_STORAGE_KEY, state.manualLang);
  sendLanguageMode();
});
els.fontSize.addEventListener("input", () => { state.fontSize = Number(els.fontSize.value); applySettings(); });
els.bottomOffset.addEventListener("input", () => { state.bottomOffset = Number(els.bottomOffset.value); applySettings(); });
els.sideMargin.addEventListener("input", () => { state.sideMargin = Number(els.sideMargin.value); applySettings(); });
els.lineHeight.addEventListener("input", () => { state.lineHeight = Number(els.lineHeight.value) / 100; applySettings(); });
els.textAlign.addEventListener("change", () => { state.textAlign = els.textAlign.value; applySettings(); });
els.charScale.addEventListener("input", () => { state.charScale = Number(els.charScale.value); applySettings(); });
els.saveGlossary.addEventListener("click", () => {
  state.glossaryTerms = parseGlossaryTermsFromText(els.glossaryTerms.value);
  els.glossaryTerms.value = state.glossaryTerms.join("\n");
  localStorage.setItem(GLOSSARY_STORAGE_KEY, els.glossaryTerms.value);
  sendGlossaryTerms();
  setStatus(`Словарь обновлен: ${state.glossaryTerms.length}`);
});

window.addEventListener("resize", reflowBuffers);

applySettings();
const storedLangMode = localStorage.getItem(LANG_MODE_STORAGE_KEY);
const storedManualLang = localStorage.getItem(MANUAL_LANG_STORAGE_KEY);
const storedGlossaryRaw = localStorage.getItem(GLOSSARY_STORAGE_KEY) || "";
if (storedLangMode === "auto" || storedLangMode === "manual") {
  state.languageMode = storedLangMode;
}
if (storedManualLang && ["kazakh", "russian", "english"].includes(storedManualLang)) {
  state.manualLang = storedManualLang;
}
state.glossaryTerms = parseGlossaryTermsFromText(storedGlossaryRaw);
els.langMode.value = state.languageMode;
els.manualLang.value = state.manualLang;
els.manualLang.disabled = state.languageMode !== "manual";
els.glossaryTerms.value = state.glossaryTerms.join("\n");
refreshMicrophoneList();
