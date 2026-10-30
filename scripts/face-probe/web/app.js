"use strict";

const $ = (id) => document.getElementById(id);
let state = null;
let busy = false;
let polling = false;
let selection = "";
let imagePath = "";
let historyKey = "";
let connectionError = false;

const labels = {
  running: "Сбор работает", stopping: "Останавливается…", stopped: "Остановлен",
  failed: "Ошибка", interrupted: "Прерван перезапуском", faces: "Найдены лица",
  heartbeat: "Состояние", stream_connecting: "Подключение", stream_connected: "Поток подключён",
  stream_retry: "Переподключение", stream_disconnected: "Поток прерван", stream_stalled: "Нет новых кадров",
  decode_error: "Ошибка кадра", storage_limit: "Лимит хранения",
};
const timeText = (value) => value ? new Date(value).toLocaleString("ru-RU") : "—";
const scoreText = (value) => Number.isFinite(value) ? value.toFixed(6) : "—";
function maxScore(event) {
  const scores = (event?.faces || []).flatMap((face) => (face.comparisons || []).map((item) => item.maxCosine)).filter(Number.isFinite);
  return scores.length ? Math.max(...scores) : null;
}
function reportURL(runId, file) {
  return `/reports/${encodeURIComponent(runId)}/${file.split("/").map(encodeURIComponent).join("/")}`;
}
function showError(message) {
  $("error").textContent = message || "";
  $("error").hidden = !message;
}
function setTheme(value) {
  document.documentElement.dataset.theme = value;
  $("theme").value = value;
  try { localStorage.setItem("face-probe-theme", value); } catch { /* Storage may be disabled. */ }
}
let savedTheme = "auto";
try { savedTheme = localStorage.getItem("face-probe-theme") || "auto"; } catch { /* Use system theme. */ }
setTheme(["light", "dark", "auto"].includes(savedTheme) ? savedTheme : "auto");
$("theme").addEventListener("change", (event) => setTheme(event.target.value));

function updateControls() {
  const active = Boolean(state?.activeRun);
  $("settings").disabled = active || busy || !state;
  $("start").disabled = active || busy || !state;
  $("stop").disabled = !active || busy || Boolean(state?.stopping);
}

function render(data) {
  state = data;
  const run = data.run;
  const config = data.config;
  $("source").textContent = `${config.baseUrl} · ${config.model} + ${config.detector}`;
  $("connection").textContent = "UI подключён";
  const activeStatus = data.activeRun ? (data.stopping ? "stopping" : "running") : "stopped";
  $("worker-state").textContent = labels[activeStatus];
  if (data.activeOptions) {
    $("mode").value = data.activeOptions.mode;
    $("fps").value = data.activeOptions.fps;
    $("interval").value = data.activeOptions.snapshotInterval;
    $("duration").value = data.activeOptions.duration;
  }
  $("config-note").textContent = `0 с — без ограничения времени. Лимит запуска: ${config.maxMb} МБ; оставить на диске ${config.minFreeMb} МБ.`;
  $("reference-note").textContent = config.references.count
    ? `Эталонных фото: ${config.references.count}. Сходство считается только для лиц, прошедших фильтры качества.`
    : `Эталоны не подключены: используйте режим детекции или подключите папку photos через volume. ${config.references.error || ""}`;
  updateControls();
  const nextKey = data.runs.join("|");
  if (nextKey !== historyKey) {
    historyKey = nextKey;
    $("history").replaceChildren(new Option("Текущий / последний", ""), ...data.runs.map((id) => new Option(id, id)));
    $("history").value = selection;
  }
  $("run-time").textContent = run ? `${labels[run.service.state] || run.service.state || "Нет статуса"} · ${timeText(run.service.startedAt)}` : "Запусков пока нет";
  const events = run?.events || [];
  const recentFace = [...events].reverse().find((event) => event.type === "faces");
  const stream = [...events].reverse().find((event) => event.type.startsWith("stream_"));
  $("frames").textContent = run ? String(run.stats.analyzedFrames ?? 0) : "—";
  $("snapshots").textContent = run ? String(run.stats.snapshots ?? run.stats.savedScreenshots ?? 0) : "—";
  $("cosine").textContent = scoreText(maxScore(recentFace));
  $("stream-state").textContent = run && run.id === data.activeRun ? (labels[stream?.type] || "Загрузка модели…") : "Не собирается";
  const logs = $("logs");
  const text = run?.logs.length ? run.logs.join("\n") : "Сбор ещё не запускался или не вывел логов.";
  if (logs.textContent !== text) {
    logs.textContent = text;
    if ($("autoscroll").checked) logs.scrollTop = logs.scrollHeight;
  }
  const result = run?.summary;
  $("result").textContent = result ? `${labels[result.status] || result.status} · ${result.stopReason}${result.errorStage ? ` · ${result.errorStage}` : ""}`
    : run?.service.error || (run?.service.state === "failed" ? `Процесс завершился с кодом ${run.service.exitCode}. Смотрите лог выше.` : "Ошибки подключения повторяются автоматически. Управления дверью нет.");
  if (run?.service.logTruncated) $("result").textContent += " Файл лога достиг 1 МБ; свежие строки доступны до перезапуска сервиса.";
  for (const [id, path] of [["download-events", "capture/events.jsonl"], ["download-manifest", "capture/manifest.json"]]) {
    $(id).hidden = !run?.manifest;
    if (run?.manifest) $(id).href = reportURL(run.id, path);
  }
  renderScreenshot(run);
  renderEvents(run, events);
}

function renderScreenshot(run) {
  const event = run?.screenshot;
  const screenshot = event?.screenshot;
  const nextPath = screenshot ? reportURL(run.id, `capture/${screenshot.file}`) : "";
  $("preview-empty").hidden = Boolean(screenshot);
  $("preview").hidden = !screenshot;
  $("image-link").hidden = !screenshot;
  $("frame-details").hidden = !screenshot;
  if (nextPath !== imagePath) {
    imagePath = nextPath;
    if (screenshot) {
      $("preview").width = screenshot.width;
      $("preview").height = screenshot.height;
      $("preview").src = nextPath;
      $("image-link").href = nextPath;
      $("metadata").textContent = JSON.stringify(event, null, 2);
    } else {
      $("preview").removeAttribute("src");
      $("metadata").textContent = "";
    }
  }
  const size = screenshot ? (screenshot.bytes < 1000 ? `${screenshot.bytes} Б` : `${(screenshot.bytes / 1000).toFixed(1)} КБ`) : "";
  $("image-caption").textContent = screenshot
    ? `${timeText(event.receivedAt)} · ${screenshot.width} × ${screenshot.height} · ${size} · max cosine ${scoreText(maxScore(event))}`
    : "Исходные пиксели · lossless WebP · без наложений";
}

function renderEvents(run, events) {
  const rows = [...events].reverse().slice(0, 40).map((event) => {
    const row = document.createElement("tr");
    const description = `${labels[event.type] || event.type}${event.reason ? `: ${event.reason}` : ""}`;
    const values = [timeText(event.analyzedAt || event.at), description, event.faces?.length ?? "—",
      scoreText(maxScore(event)), event.analysisMs ?? "—"];
    for (const value of values) {
      const cell = document.createElement("td");
      cell.textContent = String(value);
      row.append(cell);
    }
    const cell = document.createElement("td");
    if (event.screenshot) {
      const link = document.createElement("a");
      link.href = reportURL(run.id, `capture/${event.screenshot.file}`);
      link.textContent = "Открыть";
      link.target = "_blank";
      link.rel = "noopener";
      cell.append(link);
    } else cell.textContent = event.snapshotSkipped === "cooldown" ? "Пауза" : "—";
    row.append(cell);
    return row;
  });
  if (!rows.length) {
    const row = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = 6;
    cell.className = "muted";
    cell.textContent = "Событий пока нет";
    row.append(cell);
    rows.push(row);
  }
  $("events").replaceChildren(...rows);
}

async function refresh() {
  if (polling) return;
  polling = true;
  try {
    const response = await fetch(`/api/state${selection ? `?run=${encodeURIComponent(selection)}` : ""}`, { signal: AbortSignal.timeout(10000) });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "Ошибка сервиса");
    if (connectionError) { showError(""); connectionError = false; }
    render(data);
  } catch (error) {
    $("connection").textContent = "Нет связи с UI-сервисом";
    showError(error.message);
    connectionError = true;
    state = null;
    updateControls();
  } finally { polling = false; }
}

async function action(path, body) {
  if (busy || !state) return;
  busy = true;
  showError("");
  updateControls();
  try {
    const response = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json", "X-Face-Probe-Token": state.csrfToken }, body: JSON.stringify(body), signal: AbortSignal.timeout(20000) });
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "Не удалось выполнить действие");
    if (path === "/api/start") { selection = ""; $("history").value = ""; }
  } catch (error) { showError(error.message); }
  finally { busy = false; await refresh(); updateControls(); }
}
$("capture-form").addEventListener("submit", (event) => {
  event.preventDefault();
  action("/api/start", { mode: $("mode").value, fps: Number($("fps").value), snapshotInterval: Number($("interval").value), duration: Number($("duration").value) });
});
$("stop").addEventListener("click", () => action("/api/stop", {}));
$("history").addEventListener("change", (event) => { selection = event.target.value; refresh(); });
$("preview").addEventListener("error", () => showError("Не удалось загрузить скрин. Возможно, файл перемещён или удалён."));
refresh();
setInterval(() => { if (!document.hidden) refresh(); }, 2000);
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
