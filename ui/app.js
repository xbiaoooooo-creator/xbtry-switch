// XBTry wizard. Talks to Rust through window.__TAURI__ (withGlobalTauri).
"use strict";
const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const root = document.documentElement;
const NAMES = { claude: "Claude Code", codex: "Codex" };
const LAST = 3;

const store = {
  get: (k, d) => { try { return localStorage.getItem(k) ?? d; } catch { return d; } },
  set: (k, v) => { try { localStorage.setItem(k, v); } catch {} },
};

const state = {
  step: 0,
  reached: 0,       // furthest step unlocked; rail buttons beyond it are disabled
  client: null,
  models: [],
  fallback: false,
  picked: new Set(), // Codex: models added to the catalog
  def: null,         // default model for the chosen client
  status: {},
  done: false,
  busy: false,
};

// ---------- helpers ----------
function say(text, kind = "") { const m = $("#msg"); m.textContent = text; m.className = "msg " + kind; }
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const mask = (k) => (k.length > 8 ? k.slice(0, 3) + "••••" + k.slice(-4) : "••••");
const isClaudeFamily = (m) => /claude|opus|sonnet|haiku/i.test(m);

function setTone(tone) {
  root.dataset.tone = tone;
  $("#icMoon").hidden = tone !== "dark";
  $("#icSun").hidden = tone === "dark";
  store.set("xbtry-tone", tone);
}

// ---------- navigation ----------
function render() {
  $$(".pane").forEach((p) => (p.hidden = Number(p.dataset.pane) !== state.step));
  $$(".st").forEach((b) => {
    const i = Number(b.dataset.step);
    b.disabled = i > state.reached || state.busy;
    b.classList.toggle("done", i < state.step || (state.done && i === LAST));
    if (i === state.step) b.setAttribute("aria-current", "step"); else b.removeAttribute("aria-current");
  });
  $("#prev").hidden = state.step === 0;
  $("#prev").disabled = state.busy;
  const next = $("#next");
  next.disabled = state.busy || !canLeave(state.step);
  next.textContent = state.busy ? "请稍候…"
    : state.step === 1 ? "获取可用模型"
    : state.step === LAST ? (state.done ? "完成" : "一键导入")
    : "下一步";
  if (state.step === 2) renderModels();
  if (state.step === LAST) renderSummary();
  $("#panel").scrollTop = 0;
}

function canLeave(step) {
  if (step === 0) return !!state.client;
  if (step === 1) return $("#key").value.trim().length > 0;
  if (step === 2) return !!state.def;
  return true;
}

function go(step) {
  state.step = step;
  state.reached = Math.max(state.reached, step);
  say("");
  render();
}

async function onNext() {
  if (state.step === 1) { await loadModels(); return; }
  if (state.step === LAST) { state.done ? reset() : await doImport(); return; }
  go(state.step + 1);
}

// Changing an earlier answer invalidates later steps.
function invalidateFrom(step) {
  state.reached = Math.min(state.reached, step);
  state.done = false;
  $("#result").hidden = true;
}

// ---------- step 1 ----------
function chooseClient(t) {
  if (state.client !== t) invalidateFrom(state.step === 0 ? 0 : state.step);
  state.client = t;
  root.dataset.mode = t;
  $("#modeName").textContent = t === "claude" ? "CLAUDE CODE" : "CODEX";
  $("#chipClient").textContent = NAMES[t];
  store.set("xbtry-client", t);
  if (state.models.length) pickDefault();
  render();
}

async function refreshStatus() {
  let list = [];
  try { list = await invoke("status"); } catch (e) { say(String(e), "err"); return; }
  for (const s of list) {
    state.status[s.target] = s;
    const tag = $(`.state[data-state="${s.target}"]`);
    tag.classList.toggle("ok", s.connected);
    tag.textContent = s.connected ? `已接入 · ${s.model ?? ""} ${s.key_hint ?? ""}`
      : s.installed ? "已检测到，未接入" : "未检测到配置，导入后自动创建";
  }
}

// ---------- step 2 ----------
async function loadModels() {
  const key = $("#key").value.trim();
  if (/\s/.test(key)) { say("密钥中包含空格或换行，请重新复制", "err"); return; }
  state.busy = true; render(); say("正在读取模型列表…");
  try {
    const res = await invoke("fetch_models", { key });
    state.models = res.models;
    state.fallback = res.fallback;
    state.picked = new Set(res.models);
    pickDefault();
    state.busy = false;
    go(2);
    if (res.fallback) say(res.message, "err");
  } catch (e) {
    state.busy = false; render();
    say(String(e), "err");
  }
}

// ---------- step 3 ----------
function pickDefault() {
  const t = state.client;
  const saved = store.get(`xbtry-model-${t}`, "");
  const fam = state.models.find((m) => (t === "claude" ? isClaudeFamily(m) : /gpt|codex/i.test(m)));
  state.def = state.models.includes(saved) ? saved : fam ?? state.models[0] ?? null;
  if (state.def) state.picked.add(state.def);
}

function renderModels() {
  const codex = state.client === "codex";
  $("#modelLead").textContent = codex
    ? "勾选要出现在 Codex 模型菜单里的模型，并设一个默认。Claude 系模型同样可以在 Codex 中使用。"
    : "选一个模型作为 Claude Code 的默认模型，后台的 haiku / sonnet / opus 请求也会统一使用它。GPT 系模型同样可用。";
  $("#modelHint").textContent = state.fallback ? "未能联网获取，显示的是默认列表"
    : codex ? `共 ${state.models.length} 个，已选 ${state.picked.size} 个` : `共 ${state.models.length} 个`;
  $("#bulk").hidden = !codex;
  const shown = shownModels();
  // Tri-state "select all": checked = all visible picked, dash = some picked.
  const nPicked = shown.filter((m) => state.picked.has(m)).length;
  const all = $("#selAll");
  all.checked = shown.length > 0 && nPicked === shown.length;
  all.indeterminate = nPicked > 0 && nPicked < shown.length;
  all.disabled = !shown.length;
  const list = $("#mlist");
  list.replaceChildren(...shown.map((m) => {
    const li = el("li", m === state.def ? "def" : "");
    const id = "m-" + m.replace(/[^\w-]/g, "_");
    if (codex) {
      const cb = el("input"); cb.type = "checkbox"; cb.id = id; cb.checked = state.picked.has(m);
      cb.disabled = m === state.def;
      cb.addEventListener("change", () => { cb.checked ? state.picked.add(m) : state.picked.delete(m); renderModels(); });
      li.append(cb);
    } else {
      const rb = el("input"); rb.type = "radio"; rb.name = "def"; rb.id = id; rb.checked = m === state.def;
      rb.addEventListener("change", () => setDef(m));
      li.append(rb);
    }
    const lab = el("label", "nm", m); lab.htmlFor = id; li.append(lab);
    li.append(el("span", "fam", isClaudeFamily(m) ? "Claude" : /gpt|codex|^o\d/i.test(m) ? "GPT" : "其他"));
    const b = el("button", "defbtn", m === state.def ? "默认" : "设为默认"); b.type = "button";
    b.addEventListener("click", () => setDef(m));
    li.append(b);
    return li;
  }));
  if (!shown.length) list.append(el("li", "empty", "没有匹配的模型"));
}

// Models matching the search box; bulk buttons act on these only.
function shownModels() {
  const q = $("#filter").value.trim().toLowerCase();
  return state.models.filter((m) => !q || m.toLowerCase().includes(q));
}

// Select or clear every visible model. The default model always stays picked.
function bulkPick(on) {
  for (const m of shownModels()) on ? state.picked.add(m) : state.picked.delete(m);
  if (state.def) state.picked.add(state.def);
  renderModels();
}

function setDef(m) {
  state.def = m;
  state.picked.add(m);
  invalidateFrom(2);
  render();
}

// ---------- step 4 ----------
function renderSummary() {
  const key = $("#key").value.trim();
  const st = state.status[state.client];
  const rows = [
    ["中转站", "api.xbtry.com"],
    ["客户端", NAMES[state.client]],
    ["API Key", mask(key)],
    ["默认模型", state.def],
  ];
  if (state.client === "codex") rows.push(["可选模型", `${state.picked.size} 个`]);
  rows.push(["写入位置", st?.config_path ?? "—"]);
  $("#sum").replaceChildren(...rows.flatMap(([k, v]) => [el("dt", "", k), el("dd", "", v)]));
  $("#confirmTitle").textContent = state.done ? "导入完成" : "确认并导入";
  $("#confirmNote").hidden = state.done;
}

async function doImport() {
  const key = $("#key").value.trim();
  const t = state.client;
  const req = {
    key,
    claude_model: t === "claude" ? state.def : null,
    codex_model: t === "codex" ? state.def : null,
    models: t === "codex" ? state.models.filter((m) => state.picked.has(m)) : [],
  };
  state.busy = true; render(); say("正在写入配置…");
  const box = $("#result");
  try {
    const [r] = await invoke("import_config", { req });
    box.hidden = false;
    box.className = "result " + (r.ok ? "ok" : "err");
    if (r.ok) {
      state.done = true;
      store.set(`xbtry-model-${t}`, state.def);
      box.replaceChildren(el("b", "", "✓ " + r.message));
      const ol = el("ol");
      ol.append(el("li", "", `完全退出 ${NAMES[t]}（托盘里的也要退出）`), el("li", "", "重新打开，新建一个会话"), el("li", "", "发一条消息，确认能正常回复"));
      box.append(ol);
      say("");
    } else {
      box.replaceChildren(el("b", "", "✗ 导入失败"), el("span", "muted", r.message));
      say("配置没有改动，可以修正后重试", "err");
    }
  } catch (e) {
    box.hidden = true;
    say(String(e), "err");
  }
  state.busy = false;
  render();
  refreshStatus();
}

function reset() {
  state.done = false;
  state.reached = 0;
  $("#result").hidden = true;
  go(0);
}

// ---------- restore dialog ----------
async function openRestore() {
  await refreshStatus();
  const sel = $("#rdTarget");
  const opts = ["codex", "claude"].map((t) => {
    const o = new Option(NAMES[t] + (state.status[t]?.has_backup ? "" : "（无备份）"), t);
    o.disabled = !state.status[t]?.has_backup;
    return o;
  });
  sel.replaceChildren(...opts);
  const first = opts.find((o) => !o.disabled);
  if (first) sel.value = first.value;
  $("#rdGo").disabled = !first;
  $("#rdMsg").textContent = first ? "" : "还没有导入过，无需恢复。";
  $("#rdMsg").className = "msg";
  $("#restoreDlg").showModal();
}

async function doRestore() {
  const target = $("#rdTarget").value;
  const m = $("#rdMsg");
  try {
    m.textContent = await invoke("restore_config", { target });
    m.className = "msg ok";
    $("#rdGo").disabled = true;
  } catch (e) { m.textContent = String(e); m.className = "msg err"; }
  refreshStatus();
}

// ---------- wiring ----------
$$(".st").forEach((b) => b.addEventListener("click", () => go(Number(b.dataset.step))));
$$('input[name="client"]').forEach((r) => r.addEventListener("change", () => chooseClient(r.value)));
$("#next").addEventListener("click", onNext);
$("#prev").addEventListener("click", () => go(state.step - 1));
$("#toneBtn").addEventListener("click", () => setTone(root.dataset.tone === "dark" ? "light" : "dark"));
$("#openSite").addEventListener("click", () => T.opener.openUrl("https://xbtry.com"));
$("#openRestore").addEventListener("click", openRestore);
$("#rdGo").addEventListener("click", doRestore);
$("#filter").addEventListener("input", renderModels);
$("#selAll").addEventListener("change", (e) => bulkPick(e.currentTarget.checked));
$("#key").addEventListener("input", () => { invalidateFrom(1); render(); });
$("#key").addEventListener("keydown", (e) => { if (e.key === "Enter" && canLeave(1)) onNext(); });
$("#toggleKey").addEventListener("click", (e) => {
  const show = $("#key").type === "password";
  $("#key").type = show ? "text" : "password";
  e.currentTarget.textContent = show ? "隐藏" : "显示";
  e.currentTarget.setAttribute("aria-pressed", String(show));
});

setTone(store.get("xbtry-tone", "dark"));
const saved = store.get("xbtry-client", "");
if (saved === "claude" || saved === "codex") {
  $(`input[name="client"][value="${saved}"]`).checked = true;
  chooseClient(saved);
} else render();
refreshStatus();
