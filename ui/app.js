// XBTry 2.2. Talks to Rust through window.__TAURI__ (withGlobalTauri).
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
  view: "home",
  step: 0,
  reached: 0,       // furthest wizard step unlocked
  client: null,
  models: [],
  fallback: false,
  picked: new Set(), // Codex: models added to the catalog
  def: null,         // default model for the chosen client
  status: {},
  history: {},
  done: false,
  busy: false,
};

// ---------- helpers ----------
function say(text, kind = "") { const m = $("#msg"); m.textContent = text; m.className = "msg " + kind; }
function note(sel, text, kind = "") { const m = $(sel); m.textContent = text; m.className = "msg " + kind; }
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const btn = (cls, text, onClick) => { const b = el("button", cls, text); b.type = "button"; b.addEventListener("click", onClick); return b; };
const mask = (k) => (k.length > 8 ? k.slice(0, 3) + "••••" + k.slice(-4) : "••••");
const isClaudeFamily = (m) => /claude|opus|sonnet|haiku/i.test(m);
const pad = (n) => String(n).padStart(2, "0");
const fmtTime = (ms) => { const d = new Date(ms); return `${d.getMonth() + 1}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`; };
const rows = (sel, list) => $(sel).replaceChildren(...list.flatMap(([k, v]) => [el("dt", "", k), el("dd", "", v ?? "—")]));

// Disable a button while an async action runs, so double clicks do nothing.
async function busy(b, fn) {
  if (b) b.disabled = true;
  try { return await fn(); } finally { if (b) b.disabled = false; }
}

function setTone(tone) {
  root.dataset.tone = tone;
  $("#icMoon").hidden = tone !== "dark";
  $("#icSun").hidden = tone === "dark";
  store.set("xbtry-tone", tone);
}

// ---------- views ----------
const VIEW_LOAD = {
  proxy: () => loadProxy(),
  version: () => loadVersion(),
  home: () => loadFixes(),
  install: () => loadInstall(),
  launch: () => { runIp(); loadApps(); monRefresh(); },
};

function show(view) {
  state.view = view;
  $$(".view").forEach((v) => (v.hidden = v.dataset.view !== view));
  $$(".nv").forEach((b) => {
    if (b.dataset.view === view) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current");
  });
  $("#bar").hidden = view !== "import" || state.imTab !== "new";
  $("#modeName").textContent = view === "import" && state.client ? (state.client === "claude" ? "CLAUDE CODE" : "CODEX") : "CONNECT";
  $("#panel").scrollTop = 0;
  if (view === "import") render();
  VIEW_LOAD[view]?.();
  // Move focus to the new heading so screen readers announce the page.
  const h = view === "import" ? $(`.pane[data-pane="${state.step}"] h1`) : $(`.view[data-view="${view}"] h1`);
  h?.setAttribute("tabindex", "-1");
  h?.focus({ preventScroll: true });
}

// ---------- wizard navigation ----------
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
  $("#panel").scrollTop = 0;
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
  if (state.view === "import") $("#modeName").textContent = t === "claude" ? "CLAUDE CODE" : "CODEX";
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
    tag.textContent = s.connected ? `已接入 ${s.provider ?? ""} · ${s.model ?? ""} ${s.key_hint ?? ""}`
      : s.installed ? "已检测到，未接入" : "未检测到配置，导入后自动创建";
  }
}

// ---------- step 2 ----------
async function loadModels() {
  const key = $("#key").value.trim();
  if (/\s/.test(key)) { say("密钥中包含空格或换行，请重新复制", "err"); return; }
  state.busy = true; render(); say("正在读取模型列表…");
  try {
    const res = await invoke("fetch_models", { key, provider: curProv() });
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
// Newest, plain (no date / thinking suffix) Claude model wins; opus > sonnet > haiku.
function claudeScore(m) {
  if (/\d{8}|thinking/i.test(m)) return -1;
  const v = m.match(/(\d+)-(\d+)/);
  return (v ? Number(v[1]) * 10 + Number(v[2]) : 0) * 10 + (/opus/i.test(m) ? 3 : /sonnet/i.test(m) ? 2 : /haiku/i.test(m) ? 1 : 0);
}

function pickDefault() {
  const t = state.client;
  const saved = store.get(`xbtry-model-${t}`, "");
  const fam = t === "claude"
    ? state.models.filter(isClaudeFamily).sort((a, b) => claudeScore(b) - claudeScore(a))[0]
    : state.models.find((m) => /gpt|codex/i.test(m));
  state.def = state.models.includes(saved) ? saved : fam ?? state.models[0] ?? null;
  if (state.def) state.picked.add(state.def);
}

function renderModels() {
  const codex = state.client === "codex";
  $("#modelLead").textContent = codex
    ? "勾选要出现在 Codex 模型菜单里的模型，并设一个默认。Claude 系模型同样可以在 Codex 中使用。"
    : "选一个默认模型，终端、VS Code 和桌面版会一起配置。终端和 VS Code 也能用 GPT 系模型；桌面版只支持 Claude 系模型。";
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
    const b = btn("defbtn", m === state.def ? "默认" : "设为默认", () => setDef(m));
    b.setAttribute("aria-label", `${m} 设为默认`);
    li.append(b);
    return li;
  }));
  if (!shown.length) list.append(el("li", "empty", "没有匹配的模型"));
}

// Models matching the search box; bulk select acts on these only.
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
  const list = [
    ["服务商", `${curProv().name}（${host(curProv().claude_base)}）`],
    ["客户端", NAMES[state.client]],
    ["API Key", mask(key)],
    ["默认模型", state.def],
  ];
  if (state.client === "codex") list.push(["可选模型", `${state.picked.size} 个`]);
  if (state.client === "claude" && st?.also?.length) list.push(["同时配置", st.also.join("、")]);
  list.push(["写入位置", st?.config_path]);
  rows("#sum", list);
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
    models: t === "codex" ? state.models.filter((m) => state.picked.has(m)) : state.models,
    provider: curProv(),
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
      ol.append(el("li", "", t === "claude" ? "完全退出 Claude 桌面版、VS Code 和终端（托盘里的也要退出）" : `完全退出 ${NAMES[t]}（托盘里的也要退出）`), el("li", "", "重新打开，新建一个会话"), el("li", "", "发一条消息，确认能正常回复"));
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
// ---------- home: one-click checks ----------
const LEVEL_TEXT = { ok: "正常", info: "提示", warn: "注意", err: "问题" };
const CARD_NAMES = { import: "API 导入", proxy: "代理设置", install: "安装助手", version: "版本管理", launch: "启动 Claude" };

async function applyFix(f, msgSel) {
  const r = await invoke("apply_fix", { id: f.id, params: f.params ?? null });
  if (msgSel) note(msgSel, r, "ok");
  return r;
}

// Fake progress: creeps toward 92% through the stage texts, jumps to 100% when done.
function progress(p, stages) {
  const box = $(`#${p}Prog`), bar = $(`#${p}Bar`), txt = $(`#${p}Stage`);
  let v = 0, i = 0;
  box.hidden = false;
  const set = (n) => { v = n; bar.style.setProperty("--p", n + "%"); bar.setAttribute("aria-valuenow", String(Math.round(n))); };
  set(2); txt.textContent = stages[0];
  const timer = setInterval(() => {
    set(v + Math.max(0.4, (92 - v) * 0.06));
    const k = Math.min(stages.length - 1, Math.floor(v / (92 / stages.length)));
    if (k !== i) { i = k; txt.textContent = stages[k]; }
  }, 200);
  return (ok = true) => { clearInterval(timer); set(100); txt.textContent = ok ? "完成" : "已停止"; setTimeout(() => (box.hidden = true), 900); };
}

const CK_STAGES = ["检查 Codex / Claude 是否安装…", "读取配置文件…", "检查环境变量和代理…", "检测出口 IP 和服务状态…", "AI 正在分析原因…", "生成修复方案…"];
const ZH_STAGES = ["读取 Codex 语言设置…", "读取 Claude 语言设置…", "AI 正在检查…", "生成修改方案…"];

// Plan card: AI summary + one button per chosen fix + "全部修复".
function renderPlan(boxSel, plan, after) {
  const box = typeof boxSel === "string" ? $(boxSel) : boxSel;
  box.hidden = false;
  const head = el("div", "plan-head");
  head.append(el("span", "tag" + (plan.ai ? " ai" : ""), plan.ai ? `AI 分析 · ${plan.model} · ${plan.tokens} tokens` : "本地检测（未调用 AI）"), el("p", "", plan.summary || ""));
  const kids = [head];
  if (plan.note) {
    const n = el("p", "note plan-note", plan.note);
    if (/登录/.test(plan.note) && !state.acct?.signed_in) n.append(" ", btn("link", "登录 / 注册", () => openLogin("register")));
    kids.push(n);
  }
  const out = el("p", "msg");
  if (plan.fixes.length) {
    const ul = el("ul", "tlist");
    const run = async (f, b) => {
      try { out.textContent = await invoke("apply_fix", { id: f.id, params: f.params ?? null }); out.className = "msg ok"; b.textContent = "已完成"; b.disabled = true; return true; }
      catch (e) { out.textContent = String(e); out.className = "msg err"; return false; }
    };
    const btns = plan.fixes.map((f) => {
      const li = el("li");
      const d = el("div"); d.append(el("b", "", f.label), el("small", "", f.reason));
      const b = btn("btn ghost sm", "修复", () => busy(b, () => run(f, b)).then(() => after?.()));
      b.setAttribute("aria-label", `修复：${f.label}`);
      li.append(d, b); ul.append(li);
      return [f, b];
    });
    const all = btn("btn primary", plan.fixes.length > 1 ? `全部修复（${plan.fixes.length} 项）` : "修复", () => busy(all, async () => {
      const msgs = [];
      for (const [f, b] of btns) if (!b.disabled && (await run(f, b))) msgs.push(out.textContent);
      if (msgs.length) { out.textContent = msgs.join("；") + "。完全退出客户端后重新打开即可生效，可在修复记录里撤回"; out.className = "msg ok"; }
      after?.();
    }));
    kids.push(ul, el("div", "row"), out);
    kids[kids.length - 2].append(all);
  }
  box.replaceChildren(...kids);
}

function syncSend() {}

// Chat-style: the user must describe the problem; each turn = user bubble + AI plan.
async function runChecks() {
  const ta = $("#ckText"), text = ta.value.trim();
  if ($("#runChecks").dataset.busy) return;
  const b = $("#runChecks"), log = $("#chatLog");
  b.dataset.busy = "1";
  note("#checkSum", "");
  log.append(el("div", "bubble me", text || "一键检测"));
  ta.value = "";
  const reply = el("div", "bubble ai");
  await busy(b, async () => {
    const done = progress("ck", CK_STAGES);
    let plan;
    try { plan = await invoke("ai_check", { text: text || null }); done(); }
    catch (e) { done(false); reply.textContent = String(e); reply.classList.add("err"); log.append(reply); return; }
    renderChecks(plan.checks);
    $("#ckMore").hidden = false;
    const box = el("div", "plan");
    reply.append(box); log.append(reply);
    renderPlan(box, plan, () => { loadFixes(); refreshAcct(); });
    if (!plan.fixes.length) box.append(el("p", "note", "没有检测出你遇到的问题？在下面的框里描述一下（或粘贴报错原文），再点「一键检测」，AI 会针对你的描述再查一遍。"));
    refreshAcct();
  });
  delete b.dataset.busy;
  syncSend();
  reply.scrollIntoView({ block: "nearest" });
  ta.focus();
}

function renderChecks(list, prefix = "") {
  const box = $("#checks");
  const groups = [...new Set(list.map((c) => c.group))];
  box.replaceChildren(...groups.map((g) => {
    const sec = el("section", "cgroup");
    sec.append(el("h3", "", g));
    const ul = el("ul", "clist");
    for (const c of list.filter((x) => x.group === g)) {
      const li = el("li", "ck " + c.level);
      const badge = el("span", "lv", LEVEL_TEXT[c.level]);
      const txt = el("div", "ck-txt");
      txt.append(el("b", "", c.title), el("small", "", c.detail));
      li.append(badge, txt);
      const act = el("div", "ck-act");
      if (c.fix) {
        const fb = btn("btn primary sm", c.fix.label, () => busy(fb, async () => {
          try { note("#checkSum", await applyFix(c.fix), "ok"); fb.textContent = "已完成"; fb.disabled = true; loadFixes(); }
          catch (e) { note("#checkSum", String(e), "err"); }
        }));
        act.append(fb);
      }
      if (c.open) act.append(btn("btn ghost sm", CARD_NAMES[c.open] ?? "打开", () => show(c.open)));
      if (act.childElementCount) li.append(act);
      ul.append(li);
    }
    sec.append(ul);
    return sec;
  }));
  const n = (l) => list.filter((c) => c.level === l).length;
  const bad = n("err"), warn = n("warn");
  note("#checkSum", prefix + (bad || warn ? `发现 ${bad} 个问题、${warn} 个注意项` : `共 ${list.length} 项，全部正常`), bad ? "err" : warn ? "" : "ok");
}

// ---------- proxy ----------
async function loadProxy() {
  rows("#pxInfo", [["状态", "正在检测…"]]);
  let p;
  try { p = await invoke("detect_proxy"); } catch (e) { note("#pxMsg", String(e), "err"); return; }
  rows("#pxInfo", [
    ["系统代理", p.system ? `${p.system}（${p.system_on ? "已开启" : "未开启"}）` : "没有"],
    ["正在监听", p.listening.length ? p.listening.join("、") : "没有发现本机代理端口"],
    ["Codex", p.codex ?? "直连"],
    ["Claude Code", p.claude ?? "直连"],
  ]);
  const input = $("#pxUrl");
  if (!input.value) input.value = p.suggested ?? p.codex ?? p.claude ?? "";
}

async function setProxy(ids, b) {
  const url = $("#pxUrl").value.trim();
  if (!url) { note("#pxMsg", "请先填写代理地址", "err"); return; }
  await busy(b, async () => {
    const done = [];
    try {
      for (const id of ids) done.push(await invoke("apply_fix", { id, params: { url } }));
      note("#pxMsg", done.join("；"), "ok");
    } catch (e) { note("#pxMsg", [...done, String(e)].join("；"), "err"); }
    loadProxy();
  });
}

async function clearProxy(b) {
  await busy(b, async () => {
    try { note("#pxMsg", await invoke("apply_fix", { id: "proxy_clear", params: null }), "ok"); }
    catch (e) { note("#pxMsg", String(e), "err"); }
    loadProxy();
  });
}

async function trustCa(b) {
  const path = $("#caPath").value.trim();
  await busy(b, async () => {
    try { note("#caMsg", await invoke("apply_fix", { id: "claude_ca", params: { path } }), "ok"); }
    catch (e) { note("#caMsg", String(e), "err"); }
  });
}

// ---------- diagnose ----------
async function runDiagnose() {
  const text = $("#dgText").value;
  const list = $("#dgList");
  if (!text.trim()) { list.replaceChildren(el("li", "empty", "请先粘贴报错内容")); return; }
  const res = await invoke("diagnose", { text });
  if (!res.length) {
    list.replaceChildren(el("li", "empty", "没有匹配到已知原因。可以先运行「一键检测」，或把完整报错发给 XBTry 客服。"));
    return;
  }
  list.replaceChildren(...res.map((d) => {
    const li = el("li", "dg");
    const txt = el("div", "ck-txt");
    txt.append(el("b", "", d.title), el("small", "", d.cause));
    li.append(txt);
    const act = el("div", "ck-act");
    const out = el("span", "msg");
    if (d.fix) {
      const fb = btn("btn primary sm", d.fix.label, () => busy(fb, async () => {
        try { out.textContent = await applyFix(d.fix); out.className = "msg ok"; }
        catch (e) { out.textContent = String(e); out.className = "msg err"; }
      }));
      act.append(fb);
    }
    if (d.open) act.append(btn("btn ghost sm", CARD_NAMES[d.open] ?? "打开", () => show(d.open)));
    if (act.childElementCount) li.append(act);
    li.append(out);
    return li;
  }));
}

// ---------- install ----------
async function loadInstall() {
  try {
    const list = await invoke("run_checks", { key: null });
    const find = (t) => list.find((c) => c.group === "客户端" && c.title === t);
    for (const [id, t] of [["#inClaude", "Claude Code"], ["#inGit", "Git"], ["#inNode", "Node.js"], ["#inCodex", "Codex"]]) {
      const c = find(t);
      if (c?.level === "ok") { $(id).textContent = "已安装 · " + c.detail; $(id).classList.add("okt"); }
    }
  } catch {}
}

async function runTerminal(action, version, msgSel, b) {
  await busy(b, async () => {
    try { note(msgSel, await invoke("run_terminal", { action, version: version ?? null }), "ok"); }
    catch (e) { note(msgSel, String(e), "err"); }
  });
}

// ---------- version ----------
async function loadVersion() {
  rows("#vrInfo", [["状态", "正在读取…"]]);
  try {
    const v = await invoke("claude_version_info");
    rows("#vrInfo", [
      ["本机版本", v.installed ? `${v.installed}` : "没有检测到 Claude Code"],
      ["stable", v.stable],
      ["latest", v.latest],
      ["自动更新", v.autoupdate_off ? "已关闭" : "开启"],
      ["更新渠道", v.channel ?? "latest（默认）"],
    ]);
    $("#vrAuto").disabled = v.autoupdate_off;
    $("#vrStable").disabled = v.channel === "stable";
    if (!$("#vrVer").value && v.stable) $("#vrVer").placeholder = v.stable;
  } catch (e) { note("#vrMsg", String(e), "err"); }
}

async function versionFix(id, b) {
  await busy(b, async () => {
    try { note("#vrMsg", await invoke("apply_fix", { id, params: null }), "ok"); }
    catch (e) { note("#vrMsg", String(e), "err"); }
  });
  loadVersion();
}

// ---------- restore ----------
const TARGETS = ["codex", "claude"];
const restorable = (t) => !!state.status[t]?.has_backup || (state.history?.[t]?.length ?? 0) > 0;

async function loadHistory() {
  state.history = {};
  for (const t of TARGETS) {
    try { state.history[t] = await invoke("list_history", { target: t }); } catch { state.history[t] = []; }
  }
}

async function openRestore() {
  await refreshStatus();
  await loadHistory();
  const sel = $("#rdTarget");
  const prev = sel.value;
  const opts = TARGETS.map((t) => {
    const o = new Option(NAMES[t] + (restorable(t) ? "" : "（无记录）"), t);
    o.disabled = !restorable(t);
    return o;
  });
  sel.replaceChildren(...opts);
  const first = opts.find((o) => !o.disabled && o.value === prev) ?? opts.find((o) => !o.disabled && o.value === state.client) ?? opts.find((o) => !o.disabled) ?? opts[0];
  sel.value = first.value;
  renderRestoreList();
}

// Options: the pre-XBTry original (if backed up) + every earlier import.
function renderRestoreList() {
  const t = $("#rdTarget").value;
  const st = state.status[t];
  const items = [];
  if (st?.has_backup) items.push({ value: "backup", title: "原配置", sub: "恢复到 XBTry 导入之前的样子" });
  (state.history?.[t] ?? []).forEach((h, i) => {
    const current = i === 0 && !!st?.connected && st.model === h.model && st.key_hint === h.key_hint;
    let sub = `${h.provider || "XBTry 中转站"}（${h.host || "api.xbtry.com"}） · ${fmtTime(h.time)} · 密钥 ${h.key_hint}`;
    if (t === "codex" && h.model_count) sub += ` · ${h.model_count} 个可选模型`;
    items.push({ value: h.id, title: h.model, sub, current });
  });
  const box = $("#rdList");
  const legend = el("legend", "", "恢复到");
  if (!items.length) {
    box.replaceChildren(legend, el("p", "rd-empty", "还没有导入过，无需恢复。"));
  } else {
    box.replaceChildren(legend, ...items.map((it) => {
      const lab = el("label", "rd-item");
      const r = el("input"); r.type = "radio"; r.name = "rdPick"; r.value = it.value; r.disabled = !!it.current;
      const txt = el("span");
      const b = el("b", "", it.title);
      if (it.current) b.append(el("em", "rd-cur", "当前"));
      txt.append(b, el("small", "", it.sub));
      lab.append(r, txt);
      return lab;
    }));
    const pick = box.querySelector('input[name="rdPick"]:not(:disabled)');
    if (pick) pick.checked = true;
  }
  rdButton();
}

// 原配置 → 恢复；某个 API → 导入
function rdButton() {
  const pick = $('input[name="rdPick"]:checked')?.value;
  $("#rdGo").disabled = !pick;
  $("#rdGo").textContent = !pick || pick === "backup" ? "恢复" : "导入";
}

async function doRestore() {
  const target = $("#rdTarget").value;
  const pick = $('input[name="rdPick"]:checked')?.value;
  if (!pick) return;
  $("#rdGo").disabled = true;
  try {
    note("#rdMsg", pick === "backup"
      ? await invoke("restore_config", { target })
      : await invoke("apply_history", { target, id: pick }), "ok");
  } catch (e) { note("#rdMsg", String(e), "err"); }
  await refreshStatus();
  await loadHistory();
  renderRestoreList();
}

async function loadFixes() {
  const ul = $("#fixList");
  let list = [];
  try { list = await invoke("list_fixes"); } catch (e) { note("#fxMsg", String(e), "err"); }
  if (!list.length) { ul.replaceChildren(el("li", "empty", "还没有做过一键修复。")); return; }
  ul.replaceChildren(...list.map((f) => {
    const li = el("li");
    const d = el("div");
    d.append(el("b", "", f.title), el("small", "", `${fmtTime(f.time)} · ${f.files.join("、")}`));
    const b = btn("btn ghost sm", "撤回", () => busy(b, async () => {
      try { note("#fxMsg", await invoke("undo_fix", { id: f.id }), "ok"); }
      catch (e) { note("#fxMsg", String(e), "err"); }
      loadFixes();
    }));
    b.setAttribute("aria-label", `撤回 ${f.title}`);
    li.append(d, b);
    return li;
  }));
}

// ---------- launch: exit IP + Claude ----------
state.ipLevel = null;

async function runIp() {
  const b = $("#ipGo");
  note("#ipSum", "正在检测出口 IP…");
  await busy(b, async () => {
    let rep;
    try { rep = await invoke("check_ip"); } catch (e) { note("#ipSum", String(e), "err"); return; }
    state.ipLevel = rep.level;
    $("#ipList").replaceChildren(...rep.routes.map((r) => {
      const li = el("li", "ck " + r.level);
      const txt = el("div", "ck-txt");
      const where = [r.ip, r.location, r.asn, r.org].filter(Boolean).join(" · ");
      txt.append(el("b", "", r.route), el("small", "", (where || "—") + " — " + r.note));
      li.append(el("span", "lv", LEVEL_TEXT[r.level]), txt);
      return li;
    }));
    note("#ipSum", rep.level === "ok" ? "出口正常，可以启动" : "出口有问题，建议先切换节点再启动", rep.level === "ok" ? "ok" : "err");
  });
  b.textContent = "重新检测";
}

async function loadApps() {
  const ul = $("#appList");
  let apps = [];
  try { apps = await invoke("find_claude"); } catch (e) { note("#lnMsg", String(e), "err"); }
  if (!apps.length) {
    ul.replaceChildren(el("li", "empty", "没有找到 Claude。可以到「安装助手」安装 Claude Code，桌面版从 claude.com 下载。"));
    return;
  }
  ul.replaceChildren(...apps.map((a) => {
    const li = el("li");
    const d = el("div");
    d.append(el("b", "", a.label), el("small", "", a.path));
    const b = btn("btn primary sm", "启动", () => busy(b, async () => {
      if (state.ipLevel !== "ok" && !confirm(state.ipLevel ? "出口 IP 检测有问题，继续启动可能导致封号。仍然启动？" : "还没检测出口 IP，仍然启动？")) return;
      if ($("#monAuto").checked && !state.mon?.running && !(await monStart())) return;
      try { note("#lnMsg", await invoke("launch_claude", { path: a.path }), "ok"); }
      catch (e) { note("#lnMsg", String(e), "err"); }
    }));
    b.setAttribute("aria-label", `启动 ${a.label}`);
    li.append(d, b);
    return li;
  }));
}

// ---------- live IP monitor ----------
state.mon = null;
const monSec = () => Math.min(3600, Math.max(1, Math.round(Number($("#monSec").value) || 1)));

function renderMon(s) {
  state.mon = s;
  $("#monGo").disabled = !!s?.running;
  $("#monStop").disabled = !s?.running;
  if (!s?.running) { note("#monMsg", s?.checks ? "监测已停止" : ""); }
  else {
    const t = s.last_at ? new Date(s.last_at).toLocaleTimeString() : "";
    const txt = `监测中 · ${s.route} · 基准 ${s.baseline.join(" / ")} · 当前 ${s.last_ip ?? "—"} · 每 ${s.interval} 秒 · 已检测 ${s.checks} 次 ${t}` + (s.fails ? ` · 连续失败 ${s.fails} 次` : "");
    note("#monMsg", txt, s.tripped ? "err" : "ok");
  }
  const a = $("#monAlert");
  a.hidden = !s?.alert;
  if (s?.alert) { a.textContent = `${new Date(s.alert_at).toLocaleTimeString()} ${s.alert}`; a.classList.toggle("bad", !!s.tripped); }
}

async function monStart() {
  note("#monMsg", "正在记录当前出口 IP…");
  try { renderMon(await invoke("monitor_start", { interval: monSec(), killCli: $("#monCli").checked })); return true; }
  catch (e) { note("#monMsg", String(e), "err"); return false; }
}

async function monRefresh() { try { renderMon(await invoke("monitor_status")); } catch {} }

// ---------- zh ----------
async function runZh() {
  const b = $("#zhAll");
  note("#zhMsg", "");
  $("#zhPlan").hidden = true;
  await busy(b, async () => {
    const done = progress("zh", ZH_STAGES);
    let plan;
    try { plan = await invoke("ai_zh"); done(); }
    catch (e) { done(false); note("#zhMsg", String(e), "err"); return; }
    renderPlan("#zhPlan", plan, () => { loadFixes(); refreshAcct(); });
    refreshAcct();
  });
}

async function zhFix(id, b) {
  await busy(b, async () => {
    try { note("#zhMsg", await invoke("apply_fix", { id, params: null }) + "。请完全退出客户端后重新打开", "ok"); }
    catch (e) { note("#zhMsg", String(e), "err"); }
  });
}

// ---------- account (same database as api.xbtry.com) ----------
state.acct = null;
state.imTab = "new";
const lg = { mode: "password", captchaId: "", challenge: "" };
const fmtCny = (c) => "¥" + (c / 100).toFixed(2);

function renderAcct(a) {
  state.acct = a;
  const on = !!a?.signed_in;
  $("#lgOpen").hidden = on;
  $("#userChip").hidden = !on;
  if (on) {
    const [u, d] = (a.email || "").split("@");
    $("#userMail").textContent = d ? `${u.slice(0, 1)}***@${d}` : a.email;
    $("#userBal").textContent = "余额 " + fmtCny(a.balance_cents || 0);
  }
  $("#acctKeyRow").hidden = !(on && (a.has_gpt_key || a.has_claude_key)) || !isXb(curProv());
}

async function refreshAcct() {
  try { renderAcct(await invoke("account_me")); } catch { /* offline: keep last state */ }
}

async function loadCaptcha() {
  try { const c = await invoke("account_captcha"); lg.captchaId = c.id; lg.challenge = ""; $("#capImg").src = c.image; $("#capInput").value = ""; }
  catch (e) { note("#lgMsg", String(e), "err"); }
}

function setLgMode(m) {
  lg.mode = m;
  const reg = m === "register";
  $$("#lgTabs button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.mode === m)));
  $("#fCap").hidden = m === "password";
  $("#fCode").hidden = m === "password";
  $("#fPw").hidden = m === "code";
  $("#fInvite").hidden = !reg;
  $("#pwLabel").textContent = reg ? "设置密码" : "密码";
  $("#lgPw").autocomplete = reg ? "new-password" : "current-password";
  $("#lgTitle").textContent = reg ? "注册 XBTry" : "登录 XBTry";
  $("#lgSub").textContent = reg ? "填写邀请码可领 ¥1，注册后 AI 检测和一键中文立即可用" : "和 api.xbtry.com 是同一个账号";
  $("#lgSubmit").textContent = reg ? "注册并登录" : "登录";
  note("#lgMsg", "");
  if (m !== "password") loadCaptcha();
}

function openLogin(mode = "password") {
  setLgMode(mode);
  $("#lgDlg").showModal();
  $("#lgEmail").focus();
}

async function sendCode(b) {
  const email = $("#lgEmail").value.trim();
  if (!/^\d{4}$/.test($("#capInput").value.trim())) { note("#lgMsg", "请先填写图中的 4 位数字", "err"); return; }
  await busy(b, async () => {
    try {
      lg.challenge = await invoke("account_send_code", { email, purpose: lg.mode === "register" ? "register" : "login", captchaId: lg.captchaId, captcha: $("#capInput").value });
      note("#lgMsg", "验证码已发送到邮箱，5 分钟内有效", "ok");
      let s = 60; b.textContent = `${s} 秒后重发`;
      const t = setInterval(() => { s--; if (s <= 0) { clearInterval(t); b.disabled = false; b.textContent = "重新发送"; } else { b.disabled = true; b.textContent = `${s} 秒后重发`; } }, 1000);
    } catch (e) { note("#lgMsg", String(e), "err"); loadCaptcha(); }
  });
}

async function submitLogin(e) {
  e.preventDefault();
  const b = $("#lgSubmit");
  if (lg.mode !== "password" && !lg.challenge) { note("#lgMsg", "请先发送邮箱验证码", "err"); return; }
  note("#lgMsg", lg.mode === "register" ? "正在注册…" : "正在登录…");
  await busy(b, async () => {
    try {
      const a = await invoke("account_auth", { req: { mode: lg.mode, email: $("#lgEmail").value, password: $("#lgPw").value, code: $("#lgCode").value, challenge_id: lg.challenge, invite: $("#lgInvite").value } });
      renderAcct(a);
      $("#lgForm").reset();
      $("#lgDlg").close();
      const tip = a.balance_cents > 0 ? `，余额 ${fmtCny(a.balance_cents)}，AI 检测已可用` : "。余额为 0，充值后才能用 AI 检测";
      note("#checkSum", (lg.mode === "register" ? "注册成功" : "登录成功") + tip, a.balance_cents > 0 ? "ok" : "err");
    } catch (err) { note("#lgMsg", String(err), "err"); }
  });
}

async function useAcctKey(b) {
  await busy(b, async () => {
    try {
      const k = await invoke("account_keys");
      const key = state.client === "claude" ? (k.claude || k.gpt) : (k.gpt || k.claude);
      if (!key) { say("账号里还没有密钥", "err"); return; }
      $("#key").value = key; invalidateFrom(1); render();
      say("已填入账号密钥", "ok");
    } catch (e) { say(String(e), "err"); }
  });
}

function setImTab(tab) {
  state.imTab = tab;
  $$("#imTabs button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.tab === tab)));
  $("#imNew").hidden = tab !== "new";
  $("#imSaved").hidden = tab !== "saved";
  $("#bar").hidden = tab !== "new";
  if (tab === "saved") openRestore(); else render();
}

// ---------- providers ----------
state.provs = [];
const host = (u) => (u || "").replace(/^[a-z]+:\/\//, "").split("/")[0];
const curProv = () => state.provs[Number($("#provSel").value)] ?? state.provs[0] ?? { name: "XBTry 中转站", claude_base: "https://api.xbtry.com", codex_base: "https://api.xbtry.com/v1" };
const isXb = (p) => p.claude_base === "https://api.xbtry.com";

async function loadProvs(selectBase) {
  try { state.provs = await invoke("list_providers"); } catch { state.provs = []; }
  const sel = $("#provSel");
  const opts = state.provs.map((p, i) => new Option(`${p.name}（${host(p.claude_base)}）`, String(i)));
  opts.push(new Option("＋ 添加其他服务商…", "add"));
  sel.replaceChildren(...opts);
  const i = state.provs.findIndex((p) => p.claude_base === selectBase);
  sel.value = String(i >= 0 ? i : 0);
  onProv();
}

function onProv() {
  const add = $("#provSel").value === "add";
  $("#provForm").hidden = !add;
  const p = add ? null : curProv();
  $("#provDel").hidden = !p || isXb(p);
  $("#chipProv").textContent = p ? host(p.claude_base) : "新服务商";
  const xb = !!p && isXb(p);
  $("#openSite").parentElement.hidden = !xb;
  $("#acctKeyRow").hidden = !xb || !(state.acct?.signed_in && (state.acct.has_gpt_key || state.acct.has_claude_key));
  invalidateFrom(1); render();
}

async function saveProv(b) {
  await busy(b, async () => {
    try {
      const p = await invoke("save_provider", { provider: { name: $("#provName").value, claude_base: $("#provUrl").value, codex_base: "" } });
      note("#provMsg", "已保存", "ok");
      $("#provName").value = ""; $("#provUrl").value = "";
      await loadProvs(p.claude_base);
    } catch (e) { note("#provMsg", String(e), "err"); }
  });
}

async function delProv(b) {
  const p = curProv();
  if (!p || isXb(p) || !confirm(`删除服务商「${p.name}」？已导入的配置不受影响。`)) return;
  await busy(b, async () => { try { await invoke("delete_provider", { claudeBase: p.claude_base }); } catch (e) { say(String(e), "err"); } await loadProvs(); });
}

// ---------- wiring ----------
$$(".nv").forEach((b) => b.addEventListener("click", () => show(b.dataset.view)));
$$("[data-go]").forEach((b) => b.addEventListener("click", () => show(b.dataset.go)));
$("#lgOpen").addEventListener("click", () => openLogin("password"));
$("#lgOut").addEventListener("click", async () => { try { await invoke("account_logout"); } catch {} renderAcct(null); });
$$("#lgTabs button").forEach((b) => b.addEventListener("click", () => setLgMode(b.dataset.mode)));
$("#lgCancel").addEventListener("click", () => $("#lgDlg").close());
$("#lgForm").addEventListener("submit", submitLogin);
$("#sendCode").addEventListener("click", (e) => sendCode(e.currentTarget));
$("#capImg").addEventListener("click", loadCaptcha);
$("#capImg").addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); loadCaptcha(); } });
$("#useAcctKey").addEventListener("click", (e) => useAcctKey(e.currentTarget));
$("#provSel").addEventListener("change", onProv);
$("#provSave").addEventListener("click", (e) => saveProv(e.currentTarget));
$("#provDel").addEventListener("click", (e) => delProv(e.currentTarget));
$$("#imTabs button").forEach((b) => b.addEventListener("click", () => setImTab(b.dataset.tab)));
$$(".st").forEach((b) => b.addEventListener("click", () => go(Number(b.dataset.step))));
$$('input[name="client"]').forEach((r) => r.addEventListener("change", () => chooseClient(r.value)));
$("#next").addEventListener("click", onNext);
$("#prev").addEventListener("click", () => go(state.step - 1));
$("#toneBtn").addEventListener("click", () => setTone(root.dataset.tone === "dark" ? "light" : "dark"));
$("#openSite").addEventListener("click", () => T.opener.openUrl("https://xbtry.com"));
$("#ckForm").addEventListener("submit", (e) => { e.preventDefault(); runChecks(); });
$("#ckText").addEventListener("input", syncSend);
$("#ckText").addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); runChecks(); } });
$("#pxRefresh").addEventListener("click", () => { $("#pxUrl").value = ""; loadProxy(); });
$("#pxBoth").addEventListener("click", (e) => setProxy(["codex_proxy", "claude_proxy"], e.currentTarget));
$("#pxCodex").addEventListener("click", (e) => setProxy(["codex_proxy"], e.currentTarget));
$("#pxClaude").addEventListener("click", (e) => setProxy(["claude_proxy"], e.currentTarget));
$("#pxClear").addEventListener("click", (e) => clearProxy(e.currentTarget));
$("#caGo").addEventListener("click", (e) => trustCa(e.currentTarget));
$("#dgGo").addEventListener("click", runDiagnose);
$("#ipGo").addEventListener("click", () => runIp());
$("#monGo").addEventListener("click", (e) => busy(e.currentTarget, monStart).then(() => renderMon(state.mon)));
$("#monStop").addEventListener("click", async () => renderMon(await invoke("monitor_stop")));
$("#monSec").addEventListener("change", async () => { $("#monSec").value = monSec(); if (state.mon?.running) renderMon(await invoke("monitor_interval", { interval: monSec() })); });
T.event?.listen("ip-monitor", (e) => renderMon(e.payload));
$("#zhAll").addEventListener("click", () => runZh());
$$("[data-zh]").forEach((b) => b.addEventListener("click", () => zhFix(b.dataset.zh, b)));
$$("[data-act]").forEach((b) => b.addEventListener("click", () => runTerminal(b.dataset.act, null, "#inMsg", b)));
$("#inCodexGo").addEventListener("click", () => T.opener.openUrl("https://openai.com/codex/"));
$("#vrAuto").addEventListener("click", (e) => versionFix("claude_autoupdate_off", e.currentTarget));
$("#vrStable").addEventListener("click", (e) => versionFix("claude_stable", e.currentTarget));
$("#vrInstall").addEventListener("click", (e) => {
  const v = $("#vrVer").value.trim() || $("#vrVer").placeholder || "stable";
  runTerminal("claude_install_version", v, "#vrMsg", e.currentTarget);
});
$("#rdGo").addEventListener("click", doRestore);
$("#rdTarget").addEventListener("change", () => { note("#rdMsg", ""); renderRestoreList(); });
$("#rdList").addEventListener("change", rdButton);
$("#filter").addEventListener("input", renderModels);
$("#selAll").addEventListener("change", (e) => bulkPick(e.currentTarget.checked));
$("#key").addEventListener("input", () => { invalidateFrom(1); render(); });
$("#key").addEventListener("keydown", (e) => { if (e.key === "Enter" && canLeave(1)) onNext(); });
$("#toggleKey").addEventListener("click", (e) => {
  const showKey = $("#key").type === "password";
  $("#key").type = showKey ? "text" : "password";
  e.currentTarget.textContent = showKey ? "隐藏" : "显示";
  e.currentTarget.setAttribute("aria-pressed", String(showKey));
});

setTone(store.get("xbtry-tone", "dark"));
const saved = store.get("xbtry-client", "");
if (saved === "claude" || saved === "codex") {
  $(`input[name="client"][value="${saved}"]`).checked = true;
  chooseClient(saved);
}
show("home");
refreshStatus();
refreshAcct();
loadProvs();