//! Everything XBTry writes lives here: Claude Code settings, Codex config,
//! the Codex model catalog, and a one-time backup used by "恢复原配置".

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use jsonc_parser::{cst::{CstInputValue, CstRootNode}, ParseOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CLAUDE_BASE: &str = "https://api.xbtry.com";
pub const CODEX_BASE: &str = "https://api.xbtry.com/v1";
pub(crate) const PROVIDER_ID: &str = "xbtry";
/// Codex provider id for a user-added relay, so its key is never sent to XBTry.
pub(crate) const CUSTOM_PROVIDER_ID: &str = "xbtry-custom";
pub(crate) const CATALOG_FILE: &str = "xbtry-models.json";
const FALLBACK_MODELS: [&str; 3] = ["claude-opus-5-5", "gpt-6.1-sol", "gpt-6-astra"];

const GPT_TEMPLATE: &str = include_str!("../resources/gpt_template.json");
const NATIVE_TEMPLATE: &str = include_str!("../resources/native_template.json");

pub(crate) type R<T> = Result<T, String>;

pub(crate) fn home() -> R<PathBuf> {
    // XBTRY_HOME lets tests (and portable setups) redirect every path.
    if let Some(h) = env_dir("XBTRY_HOME") {
        return Ok(h);
    }
    dirs::home_dir().ok_or_else(|| "找不到当前用户目录".to_string())
}

pub(crate) fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn claude_dir() -> R<PathBuf> {
    Ok(env_dir("CLAUDE_CONFIG_DIR").unwrap_or(home()?.join(".claude")))
}

pub(crate) fn codex_dir() -> R<PathBuf> {
    Ok(env_dir("CODEX_HOME").unwrap_or(home()?.join(".codex")))
}

fn backup_root() -> R<PathBuf> {
    Ok(home()?.join(".xbtry").join("backup"))
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Claude,
    Codex,
}

impl Target {
    fn key(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Codex => "codex",
        }
    }

    /// Files XBTry may touch for this client. All of them are backed up before
    /// the first import and put back (or removed) on restore.
    fn files(self) -> R<Vec<PathBuf>> {
        Ok(match self {
            Target::Claude => vec![claude_dir()?.join("settings.json"), home()?.join(".claude.json")],
            Target::Codex => {
                let d = codex_dir()?;
                vec![d.join("config.toml"), d.join("auth.json"), d.join(CATALOG_FILE)]
            }
        })
    }
}

// ---------- file helpers ----------

pub(crate) fn read_opt(path: &Path) -> R<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
    }
}

/// Write through a temp file + rename so a crash never leaves half a config.
pub(crate) fn write_atomic(path: &Path, text: &str) -> R<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建目录 {} 失败：{e}", dir.display()))?;
    }
    let tmp = path.with_extension("xbtry-tmp");
    fs::write(&tmp, text).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("写入 {} 失败：{e}", path.display())
    })
}

pub(crate) fn read_json_object(path: &Path) -> R<Map<String, Value>> {
    let Some(text) = read_opt(path)? else {
        return Ok(Map::new());
    };
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(format!(
            "{} 不是有效的 JSON，为避免覆盖你的配置已停止。请修复或删除后重试。",
            path.display()
        )),
    }
}

pub(crate) fn write_json(path: &Path, value: &Value) -> R<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, &(text + "\n"))
}

// ---------- backup / restore ----------

#[derive(Serialize, Deserialize)]
struct BackupEntry {
    path: PathBuf,
    /// File name inside the backup folder; `None` means the file did not exist.
    saved_as: Option<String>,
}

fn backup_dir(t: Target) -> R<PathBuf> {
    Ok(backup_root()?.join(t.key()))
}

fn has_backup(t: Target) -> bool {
    backup_dir(t)
        .map(|d| d.join("manifest.json").is_file())
        .unwrap_or(false)
}

/// Keep the state from *before* XBTry: a file is backed up the first time
/// XBTry is about to write it and never again, so re-imports keep the original.
/// Files added in later versions (desktop, VS Code) join the same manifest.
pub(crate) fn ensure_backup(t: Target, paths: &[PathBuf]) -> R<()> {
    let dir = backup_dir(t)?;
    let manifest = dir.join("manifest.json");
    let mut entries: Vec<BackupEntry> = match read_opt(&manifest)? {
        Some(text) => serde_json::from_str(&text)
            .map_err(|_| "备份记录已损坏，为避免丢失原配置已停止导入。".to_string())?,
        None => Vec::new(),
    };
    let known = entries.len();
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    for path in paths {
        if entries.iter().any(|e| &e.path == path) {
            continue;
        }
        let saved_as = if path.is_file() {
            let name = format!("{}-{}", entries.len(), path.file_name().unwrap_or_default().to_string_lossy());
            fs::copy(path, dir.join(&name)).map_err(|e| format!("备份 {} 失败：{e}", path.display()))?;
            Some(name)
        } else {
            None
        };
        entries.push(BackupEntry { path: path.clone(), saved_as });
    }
    if entries.len() == known && manifest.is_file() {
        return Ok(());
    }
    let value = serde_json::to_value(&entries).map_err(|e| e.to_string())?;
    write_json(&manifest, &value)
}

#[tauri::command]
pub fn restore_config(target: Target) -> R<String> {
    let dir = backup_dir(target)?;
    let manifest = dir.join("manifest.json");
    let text = read_opt(&manifest)?.ok_or("没有找到导入前的备份，无需恢复。")?;
    let entries: Vec<BackupEntry> =
        serde_json::from_str(&text).map_err(|_| "备份记录已损坏，未做任何修改。".to_string())?;
    for e in &entries {
        match &e.saved_as {
            Some(name) => {
                let data = fs::read(dir.join(name)).map_err(|err| format!("读取备份失败：{err}"))?;
                if let Some(p) = e.path.parent() {
                    let _ = fs::create_dir_all(p);
                }
                fs::write(&e.path, data).map_err(|err| format!("恢复 {} 失败：{err}", e.path.display()))?;
            }
            None => {
                if e.path.exists() {
                    fs::remove_file(&e.path).map_err(|err| format!("删除 {} 失败：{err}", e.path.display()))?;
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    Ok("已恢复到导入前的配置".into())
}

// ---------- models ----------

#[derive(Serialize)]
pub struct ModelList {
    models: Vec<String>,
    /// true when the relay could not be reached and the built-in list is used.
    fallback: bool,
    message: Option<String>,
}

#[tauri::command]
pub async fn fetch_models(key: String, provider: Option<Provider>) -> R<ModelList> {
    let prov = normalize_provider(provider)?;
    let custom = !prov.is_xbtry();
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("请先填写 API Key".into());
    }
    let fallback = |message: String| ModelList {
        models: FALLBACK_MODELS.iter().map(|s| s.to_string()).collect(),
        fallback: true,
        message: Some(message),
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| e.to_string())?;
    let res = match client.get(format!("{}/models", prov.codex_base)).bearer_auth(&key).send().await {
        Ok(r) => r,
        Err(_) if custom => return Err(format!("连不上 {}，请检查接口地址和网络", prov.name)),
        Err(_) => return Ok(fallback("网络连接失败，已使用默认模型列表".into())),
    };
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("API Key 无效或已过期，请检查后重试".into());
    }
    if !status.is_success() {
        if custom {
            return Err(format!("{} 返回 HTTP {}，没能获取模型列表。请确认接口地址支持 /v1/models", prov.name, status.as_u16()));
        }
        return Ok(fallback(format!("模型列表获取失败（HTTP {}），已使用默认列表", status.as_u16())));
    }
    let body: Value = res.json().await.unwrap_or(Value::Null);
    let list = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .cloned()
        .unwrap_or_default();
    let mut models: Vec<String> = list
        .iter()
        .filter_map(|m| m.as_str().or_else(|| m.get("id").and_then(Value::as_str)))
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect();
    models.dedup();
    if models.is_empty() {
        if custom {
            return Err(format!("{} 没有返回任何模型，请检查密钥和接口地址", prov.name));
        }
        return Ok(fallback("中转站没有返回模型，已使用默认列表".into()));
    }
    Ok(ModelList { models, fallback: false, message: None })
}

fn is_gpt_family(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("gpt") || m.contains("codex") || m.starts_with('o') && m[1..].starts_with(char::is_numeric)
}

fn pretty_name(model: &str) -> String {
    model
        .split(['-', '_'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) if p.eq_ignore_ascii_case("gpt") => format!("{}{}", f.to_ascii_uppercase(), c.as_str().to_ascii_uppercase()),
                Some(f) => format!("{}{}", f.to_ascii_uppercase(), c.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One Codex catalog row. GPT models mirror the official GPT template (with
/// apply_patch etc.); other families (Claude via the relay) use the clean
/// template that only relies on shell commands, which every gateway accepts.
fn catalog_entry(model: &str, priority: usize) -> R<Value> {
    let gpt = is_gpt_family(model);
    let mut entry: Value =
        serde_json::from_str(if gpt { GPT_TEMPLATE } else { NATIVE_TEMPLATE }).map_err(|e| e.to_string())?;
    let obj = entry.as_object_mut().ok_or("模型模板无效")?;
    let name = pretty_name(model);
    obj.insert("slug".into(), json!(model));
    obj.insert("display_name".into(), json!(name));
    obj.insert("description".into(), json!(format!("{name} · XBTry")));
    obj.insert("visibility".into(), json!("list"));
    obj.insert("priority".into(), json!(priority));
    obj.insert("additional_speed_tiers".into(), json!([]));
    obj.insert("service_tiers".into(), json!([]));
    obj.insert("availability_nux".into(), Value::Null);
    obj.insert("upgrade".into(), Value::Null);
    obj.insert("input_modalities".into(), json!(["text", "image"]));
    if !gpt {
        obj.insert(
            "supported_reasoning_levels".into(),
            json!([
                { "effort": "low", "description": "Fast responses" },
                { "effort": "medium", "description": "Balanced" },
                { "effort": "high", "description": "Deeper reasoning" }
            ]),
        );
        obj.insert("default_reasoning_level".into(), json!("medium"));
        obj.insert("context_window".into(), json!(200000));
        obj.insert("max_context_window".into(), json!(200000));
    }
    Ok(entry)
}

// ---------- providers ----------
// XBTry is the built-in provider; users can add any relay that speaks the
// Anthropic Messages API (Claude) and the OpenAI Responses API (Codex).

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Provider {
    pub name: String,
    pub claude_base: String,
    #[serde(default)]
    pub codex_base: String,
}

impl Provider {
    pub fn xbtry() -> Self {
        Provider { name: "XBTry 中转站".into(), claude_base: CLAUDE_BASE.into(), codex_base: CODEX_BASE.into() }
    }
    pub fn is_xbtry(&self) -> bool {
        self.claude_base == CLAUDE_BASE
    }
}

fn xbtry_provider() -> Provider {
    Provider::xbtry()
}

pub(crate) fn host_of(u: &str) -> String {
    u.split_once("://").map_or(u, |x| x.1).split('/').next().unwrap_or(u).to_string()
}

fn clean_base(u: &str, what: &str) -> R<String> {
    let u = u.trim().trim_end_matches('/');
    let scheme_ok = u.starts_with("https://") || u.starts_with("http://127.0.0.1") || u.starts_with("http://localhost");
    let host = host_of(u);
    let bad = u.chars().any(|c| c.is_whitespace() || "\"'\\<>`".contains(c));
    if !scheme_ok || host.is_empty() || host.contains('@') || bad || u.len() > 200 {
        return Err(format!("{what}格式不对，要以 https:// 开头，例如 https://api.example.com"));
    }
    Ok(u.to_string())
}

/// Validate a provider from the UI; `None` means XBTry.
pub(crate) fn normalize_provider(p: Option<Provider>) -> R<Provider> {
    let Some(p) = p else { return Ok(Provider::xbtry()) };
    let mut claude = clean_base(&p.claude_base, "Claude 接口地址")?;
    let mut codex = format!("{claude}/v1");
    // People often paste the /v1 address; Claude Code adds /v1/messages itself.
    if let Some(s) = claude.strip_suffix("/v1") {
        codex = claude.clone();
        claude = s.to_string();
    }
    if !p.codex_base.trim().is_empty() {
        codex = clean_base(&p.codex_base, "OpenAI 接口地址")?;
    }
    let name: String = p.name.trim().chars().filter(|c| !c.is_control()).take(30).collect();
    let name = if name.is_empty() { host_of(&claude) } else { name };
    let out = Provider { name, claude_base: claude, codex_base: codex };
    Ok(if out.is_xbtry() { Provider::xbtry() } else { out })
}

fn providers_path() -> R<PathBuf> {
    Ok(home()?.join(".xbtry").join("providers.json"))
}

fn load_providers() -> Vec<Provider> {
    providers_path()
        .ok()
        .and_then(|p| read_json_object(&p).ok())
        .and_then(|m| m.get("providers").cloned())
        .and_then(|v| serde_json::from_value::<Vec<Provider>>(v).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.is_xbtry())
        .collect()
}

fn save_providers(list: &[Provider]) -> R<()> {
    write_json(&providers_path()?, &json!({ "providers": list }))
}

/// XBTry first, then the user's own providers. No keys are stored here.
#[tauri::command]
pub fn list_providers() -> Vec<Provider> {
    let mut v = vec![Provider::xbtry()];
    v.extend(load_providers());
    v
}

#[tauri::command]
pub fn save_provider(provider: Provider) -> R<Provider> {
    let p = normalize_provider(Some(provider))?;
    if p.is_xbtry() {
        return Ok(p);
    }
    let mut list = load_providers();
    list.retain(|x| x.claude_base != p.claude_base && x.name != p.name);
    list.insert(0, p.clone());
    list.truncate(20);
    save_providers(&list)?;
    Ok(p)
}

#[tauri::command]
pub fn delete_provider(claude_base: String) -> R<()> {
    let mut list = load_providers();
    list.retain(|x| x.claude_base != claude_base.trim_end_matches('/'));
    save_providers(&list)
}

/// Display name for a base URL a client is configured with.
pub(crate) fn provider_name(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b == CLAUDE_BASE || b == CODEX_BASE {
        return Provider::xbtry().name;
    }
    load_providers()
        .into_iter()
        .find(|p| p.claude_base == b || p.codex_base == b)
        .map(|p| p.name)
        .unwrap_or_else(|| host_of(b))
}

// ---------- import ----------

#[derive(Deserialize)]
pub struct ImportRequest {
    key: String,
    claude_model: Option<String>,
    codex_model: Option<String>,
    /// Every model shown in the app; all become selectable inside Codex.
    #[serde(default)]
    models: Vec<String>,
    /// Which provider to import; missing = XBTry.
    #[serde(default)]
    provider: Option<Provider>,
}

#[derive(Serialize)]
pub struct ImportResult {
    target: Target,
    ok: bool,
    message: String,
}

fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

// ---------- Claude: CLI, VS Code extension, desktop app ----------
// Layout follows SuperGPT Connect / CC Switch (both MIT): one import covers
// every Claude entry point that is installed on this machine.

/// Connection keys owned by the provider. They are cleared before writing so
/// stale credentials, model pins or cloud routing from earlier tools can't win.
const CLAUDE_FLOOR_ENV_PREFIXES: [&str; 3] = ["ANTHROPIC_", "AWS_", "VERTEX_REGION_"];
const CLAUDE_PROTOCOL_SELECTORS: [&str; 7] = [
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_GATEWAY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
];
const CLAUDE_FLOOR_ENV_KEYS: [&str; 9] = [
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
    "ENABLE_TOOL_SEARCH",
];
const CLAUDE_FLOOR_TOP: [&str; 13] = [
    "apiKeyHelper", "apiBaseUrl", "primaryModel", "smallFastModel", "apiKey", "model",
    "fallbackModel", "modelOverrides", "modelPicker", "advisorModel", "awsAuthRefresh",
    "awsCredentialExport", "gcpAuthRefresh",
];
/// Written as empty strings: removing a JSON key does not unset a variable
/// inherited from Windows/shell, an explicit empty value does.
const CLAUDE_NEUTRALIZE: [&str; 5] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
];

fn claude_floor_env(key: &str) -> bool {
    CLAUDE_FLOOR_ENV_PREFIXES.iter().any(|p| key.starts_with(p))
        || CLAUDE_PROTOCOL_SELECTORS.contains(&key)
        || CLAUDE_FLOOR_ENV_KEYS.contains(&key)
        || (key.starts_with("CLAUDE_CODE_SKIP_") && key.ends_with("_AUTH"))
}

const DESKTOP_PROFILE_ID: &str = "4b1f0c3e-8a2d-4e7b-9c51-6d0e2f7a9b13";

/// Per-user app data roots (local, roaming). Under XBTRY_HOME they live inside
/// it, so tests never touch the real desktop or VS Code settings.
#[allow(unused_variables)]
fn app_data() -> R<(PathBuf, PathBuf)> {
    let home = home()?;
    let sandbox = env_dir("XBTRY_HOME").is_some();
    #[cfg(windows)]
    {
        let pick = |var: &str, sub: &str| {
            if sandbox { home.join(sub) } else { env_dir(var).unwrap_or_else(|| home.join(sub)) }
        };
        return Ok((pick("LOCALAPPDATA", "AppData/Local"), pick("APPDATA", "AppData/Roaming")));
    }
    #[cfg(target_os = "macos")]
    {
        let d = home.join("Library/Application Support");
        return Ok((d.clone(), d));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let d = if sandbox { home.join(".config") } else { env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")) };
        Ok((d.clone(), d))
    }
}

/// Every place a Claude desktop build reads its gateway setup, one
/// [profile, library meta, 3p config, normal config] set per install.
/// Store (MSIX) builds see %LOCALAPPDATA% through a per-package copy under
/// Packages\<family>\LocalCache, so each installed package gets its own set.
fn desktop_targets() -> R<Vec<[PathBuf; 4]>> {
    let (local, _) = app_data()?;
    let set = |threep: PathBuf, main: PathBuf| {
        [
            threep.join("configLibrary").join(format!("{DESKTOP_PROFILE_ID}.json")),
            threep.join("configLibrary").join("_meta.json"),
            threep.join("claude_desktop_config.json"),
            main,
        ]
    };
    let mut out = vec![set(local.join("Claude-3p"), local.join("Claude").join("claude_desktop_config.json"))];
    for cache in store_caches(&local) {
        out.push(set(cache.join("Local").join("Claude-3p"), cache.join("Roaming").join("Claude").join("claude_desktop_config.json")));
    }
    Ok(out)
}

/// LocalCache folders of installed Claude store packages.
pub(crate) fn store_caches(local: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(local.join("Packages"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("Claude_") || n.starts_with("AnthropicPBC.Claude_")
                })
                .map(|e| e.path().join("LocalCache"))
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn desktop_files() -> R<Vec<PathBuf>> {
    Ok(desktop_targets()?.into_iter().flatten().collect())
}

pub(crate) fn desktop_installed() -> bool {
    app_data()
        .map(|(local, _)| local.join("Claude").is_dir() || local.join("Claude-3p").is_dir() || !store_caches(&local).is_empty())
        .unwrap_or(false)
}

fn vscode_settings() -> R<PathBuf> {
    Ok(app_data()?.1.join("Code").join("User").join("settings.json"))
}

pub(crate) fn vscode_installed() -> bool {
    vscode_settings()
        .map(|p| p.parent().is_some_and(Path::is_dir))
        .unwrap_or(false)
}

/// The desktop model menu only accepts Claude family IDs.
fn desktop_model_ok(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    let id = lower.strip_prefix("anthropic/").unwrap_or(&lower);
    ["claude-sonnet-", "claude-opus-", "claude-haiku-", "claude-fable-"]
        .iter()
        .any(|p| id.strip_prefix(p).is_some_and(|t| !t.is_empty() && !t.contains('[')))
}

/// Claude desktop: a separate third-party "gateway" profile, then switch the
/// app into 3p mode. Other profiles, MCP servers and policies stay intact.
fn import_desktop(key: &str, model: &str, models: &[String], base: &str) -> R<()> {
    let targets = desktop_targets()?;
    // Validate every install first; write nothing if any file is broken.
    let plans = targets.iter().map(|t| desktop_plan(t, key, model, models, base)).collect::<R<Vec<_>>>()?;
    for (files, docs) in targets.iter().zip(plans) {
        for (p, v) in files.iter().zip(docs) {
            write_json(p, &v)?;
        }
    }
    Ok(())
}

/// New contents for one install, in write order (profile, meta, 3p, main).
fn desktop_plan(files: &[PathBuf; 4], key: &str, model: &str, models: &[String], base: &str) -> R<[Value; 4]> {
    let (profile_p, meta_p, threep_p, main_p) = (&files[0], &files[1], &files[2], &files[3]);
    // Read and validate everything first; write nothing if any file is broken.
    let mut profile = read_json_object(profile_p)?;
    let mut meta = read_json_object(meta_p)?;
    let mut threep = read_json_object(threep_p)?;
    let mut main = read_json_object(main_p)?;
    let entries = meta
        .entry("entries")
        .or_insert(json!([]))
        .as_array_mut()
        .ok_or("Claude 桌面版配置目录格式无效，桌面版未修改。")?;
    if entries.iter().any(|v| v.get("id").and_then(Value::as_str).is_none()) {
        return Err("Claude 桌面版配置目录里有无法识别的条目，桌面版未修改。".into());
    }
    match entries.iter_mut().find(|v| v["id"] == DESKTOP_PROFILE_ID) {
        Some(e) => e["name"] = json!("XBTry"),
        None => entries.push(json!({ "id": DESKTOP_PROFILE_ID, "name": "XBTry" })),
    }
    meta.insert("appliedId".into(), json!(DESKTOP_PROFILE_ID));

    let mut ordered = vec![model.to_string()];
    ordered.extend(models.iter().filter(|m| *m != model && desktop_model_ok(m)).cloned());
    for (k, v) in [
        ("inferenceProvider", json!("gateway")),
        ("inferenceGatewayBaseUrl", json!(base)),
        ("inferenceGatewayApiKey", json!(key)),
        ("inferenceGatewayAuthScheme", json!("bearer")),
        ("inferenceCredentialKind", json!("static")),
        ("modelDiscoveryEnabled", json!(false)),
        ("toolSearchEnabled", json!(false)),
        ("inferenceModels", json!(ordered)),
    ] {
        profile.insert(k.into(), v);
    }
    for cfg in [&mut threep, &mut main] {
        // Older third-party setups kept routing in enterpriseConfig; clear only those keys.
        if let Some(Value::Object(e)) = cfg.get_mut("enterpriseConfig") {
            for k in ["inferenceProvider", "inferenceGatewayBaseUrl", "inferenceGatewayApiKey", "inferenceGatewayAuthScheme"] {
                e.remove(k);
            }
        }
        cfg.insert("deploymentMode".into(), json!("3p"));
    }
    // Activate only after the credential and the library entry are in place.
    Ok([Value::Object(profile), Value::Object(meta), Value::Object(threep), Value::Object(main)])
}

fn jsonc_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

/// VS Code extension: it already reads ~/.claude/settings.json, it only needs
/// the official sign-in screen turned off. The key is not copied into VS Code
/// settings (Settings Sync would upload it). Comments and formatting are kept.
fn import_vscode() -> R<()> {
    let path = vscode_settings()?;
    let before = read_opt(&path)?.unwrap_or_default();
    let trimmed = before.trim_start_matches('\u{feff}');
    let text = if trimmed.trim().is_empty() { "{}" } else { trimmed };
    let opts = jsonc_options();
    let doc: Value = jsonc_parser::parse_to_serde_value(text, &opts)
        .map_err(|_| "VS Code 设置文件格式有误，VS Code 未修改。".to_string())?;
    if !doc.is_object() {
        return Err("VS Code 设置文件格式有误，VS Code 未修改。".into());
    }
    let root = CstRootNode::parse(text, &opts).map_err(|_| "无法解析 VS Code 设置，VS Code 未修改。".to_string())?;
    let obj = root.object_value_or_set();
    let k = "claudeCode.disableLoginPrompt";
    if obj.properties().iter().filter(|p| p.decoded_name().as_deref() == Some(k)).count() > 1 {
        return Err("VS Code 设置里有重复的 claudeCode.disableLoginPrompt，VS Code 未修改。".into());
    }
    if let Some(p) = obj.get(k) {
        p.set_value(CstInputValue::Bool(true));
    } else {
        obj.append(k, CstInputValue::Bool(true));
    }
    write_atomic(&path, &root.to_string())
}

/// What else got configured besides the terminal CLI.
#[derive(Default)]
struct ClaudeExtras {
    done: Vec<&'static str>,
    warnings: Vec<String>,
}

fn import_claude(key: &str, model: &str, models: &[String], prov: &Provider) -> R<ClaudeExtras> {
    let settings_path = claude_dir()?.join("settings.json");
    let mut settings = read_json_object(&settings_path)?;
    let state_path = home()?.join(".claude.json");
    let mut state = read_json_object(&state_path)?;
    let desktop = desktop_installed();
    let desktop_ok = desktop && desktop_model_ok(model);
    let vscode = vscode_installed();
    let mut files = Target::Claude.files()?;
    if desktop_ok {
        files.extend(desktop_files()?);
    }
    if vscode {
        files.push(vscode_settings()?);
    }
    ensure_backup(Target::Claude, &files)?;

    let mut env = match settings.remove("env") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    env.retain(|k, _| !claude_floor_env(k));
    settings.retain(|k, _| !CLAUDE_FLOOR_TOP.contains(&k.as_str()));
    for k in CLAUDE_NEUTRALIZE.iter().chain(CLAUDE_PROTOCOL_SELECTORS.iter()) {
        env.insert((*k).into(), json!(""));
    }
    env.insert("ANTHROPIC_BASE_URL".into(), json!(prov.claude_base));
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(key));
    // All aliases point at the chosen model, so a GPT model works too and
    // Claude Code's background haiku/sonnet calls never ask for a missing name.
    for k in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
    ] {
        env.insert(k.into(), json!(model));
    }
    // Gateway compatibility switches from the Claude Code docs.
    env.insert("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(), json!("1"));
    env.insert("CLAUDE_CODE_AUTO_MODE_SERVER".into(), json!("0"));
    env.insert("ENABLE_TOOL_SEARCH".into(), json!("false"));
    settings.insert("env".into(), Value::Object(env));
    settings.insert("model".into(), json!(model));
    if !models.is_empty() {
        let mut list = vec![model.to_string()];
        list.extend(models.iter().filter(|m| *m != model).cloned());
        let options: Vec<Value> = list
            .iter()
            .map(|m| json!({ "model": m, "label": m, "description": "通过 XBTry 调用" }))
            .collect();
        settings.insert("modelPicker".into(), json!({ "replaceBuiltInOptions": true, "options": options }));
        settings.insert("availableModels".into(), json!(list));
    }
    write_json(&settings_path, &Value::Object(settings))?;

    // Skip the first-run login screen; the token above is the login.
    state.insert("hasCompletedOnboarding".into(), json!(true));
    write_json(&state_path, &Value::Object(state))?;

    let mut extras = ClaudeExtras::default();
    if vscode {
        match import_vscode() {
            Ok(()) => extras.done.push("VS Code 插件"),
            Err(e) => extras.warnings.push(e),
        }
    }
    if desktop {
        if !desktop_ok {
            extras.warnings.push("Claude 桌面版只支持 claude-opus / sonnet / haiku 系模型，这次没有配置桌面版".into());
        } else {
            match import_desktop(key, model, models, &prov.claude_base) {
                Ok(()) => extras.done.push("Claude 桌面版"),
                Err(e) => extras.warnings.push(e),
            }
        }
    }
    Ok(extras)
}

fn import_codex(key: &str, model: &str, models: &[String], prov: &Provider) -> R<()> {
    let pid = if prov.is_xbtry() { PROVIDER_ID } else { CUSTOM_PROVIDER_ID };
    let dir = codex_dir()?;
    let config_path = dir.join("config.toml");
    let text = read_opt(&config_path)?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .trim_start_matches('\u{feff}')
        .parse()
        .map_err(|_| format!("{} 格式有误，为避免覆盖你的配置已停止。", config_path.display()))?;
    ensure_backup(Target::Codex, &Target::Codex.files()?)?;

    // Catalog: chosen model first, then everything else from the relay.
    let mut slugs = vec![model.to_string()];
    for m in models {
        if !slugs.contains(m) {
            slugs.push(m.clone());
        }
    }
    let entries = slugs
        .iter()
        .enumerate()
        .map(|(i, m)| catalog_entry(m, i))
        .collect::<R<Vec<_>>>()?;
    let catalog_path = dir.join(CATALOG_FILE);
    write_json(&catalog_path, &json!({ "models": entries }))?;

    doc["model_provider"] = toml_edit::value(pid);
    doc["model"] = toml_edit::value(model);
    doc["model_catalog_json"] = toml_edit::value(catalog_path.to_string_lossy().to_string());
    if doc.get("model_providers").and_then(|i| i.as_table()).is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc["model_providers"] = toml_edit::Item::Table(t);
    }
    let mut provider = toml_edit::Table::new();
    provider["name"] = toml_edit::value(prov.name.as_str());
    provider["base_url"] = toml_edit::value(prov.codex_base.as_str());
    provider["wire_api"] = toml_edit::value("responses");
    provider["requires_openai_auth"] = toml_edit::value(true);
    // Keep retry / timeout tuning from an earlier fix across re-imports.
    if let Some(old) = doc.get("model_providers").and_then(|m| m.get(pid)) {
        for k in ["stream_max_retries", "request_max_retries", "stream_idle_timeout_ms"] {
            if let Some(v) = old.get(k) {
                provider[k] = v.clone();
            }
        }
    }
    doc["model_providers"][pid] = toml_edit::Item::Table(provider);
    write_atomic(&config_path, &doc.to_string())?;

    write_json(&dir.join("auth.json"), &json!({ "OPENAI_API_KEY": key }))
}

fn claude_message(model: &str, x: &ClaudeExtras) -> String {
    let mut msg = format!("Claude Code 已切换到 {model}");
    if !x.done.is_empty() {
        msg += &format!("，同时配置了{}", x.done.join("、"));
    }
    if !x.warnings.is_empty() {
        msg += &format!("。注意：{}", x.warnings.join("；"));
    }
    msg
}

#[tauri::command]
pub fn import_config(req: ImportRequest) -> R<Vec<ImportResult>> {
    let key = req.key.trim();
    if key.is_empty() {
        return Err("请先填写 API Key".into());
    }
    if key.chars().any(char::is_whitespace) {
        return Err("API Key 中包含空格或换行，请重新复制".into());
    }
    let prov = normalize_provider(req.provider.clone())?;
    let mut out = Vec::new();
    if let Some(m) = clean(&req.claude_model) {
        let r = import_claude(key, &m, &req.models, &prov);
        if r.is_ok() {
            let _ = record_history(Target::Claude, key, &m, &req.models, &prov);
        }
        out.push(ImportResult {
            target: Target::Claude,
            ok: r.is_ok(),
            message: match r {
                Ok(x) => claude_message(&m, &x),
                Err(e) => e,
            },
        });
    }
    if let Some(m) = clean(&req.codex_model) {
        let r = import_codex(key, &m, &req.models, &prov);
        if r.is_ok() {
            let _ = record_history(Target::Codex, key, &m, &req.models, &prov);
        }
        out.push(ImportResult {
            target: Target::Codex,
            ok: r.is_ok(),
            message: r.err().unwrap_or_else(|| format!("Codex 已通过 {} 切换到 {m}", prov.name)),
        });
    }
    if out.is_empty() {
        return Err("请至少选择一个客户端".into());
    }
    Ok(out)
}

// ---------- import history ----------
// Every successful import is remembered so the restore dialog can switch
// back to it. Re-applying runs the normal import, so it merges with the
// user's current config instead of overwriting whole files.

const HISTORY_MAX: usize = 10;

#[derive(Serialize, Deserialize, Clone)]
struct HistoryEntry {
    id: String,
    time: u64,
    key: String,
    model: String,
    #[serde(default)]
    models: Vec<String>,
    /// Older records have no provider: they were XBTry.
    #[serde(default = "xbtry_provider")]
    provider: Provider,
}

/// What the UI sees: never the raw key.
#[derive(Serialize)]
pub struct HistoryItem {
    id: String,
    time: u64,
    model: String,
    model_count: usize,
    key_hint: String,
    provider: String,
    host: String,
}

fn history_path() -> R<PathBuf> {
    Ok(home()?.join(".xbtry").join("history.json"))
}

fn history_of(all: &Map<String, Value>, t: Target) -> Vec<HistoryEntry> {
    all.get(t.key())
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn record_history(t: Target, key: &str, model: &str, models: &[String], prov: &Provider) -> R<()> {
    let path = history_path()?;
    let mut all = read_json_object(&path).unwrap_or_default();
    let mut list = history_of(&all, t);
    list.retain(|e| !(e.key == key && e.model == model && e.models == models && e.provider == *prov));
    let mut time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    while list.iter().any(|e| e.time == time) {
        time += 1;
    }
    list.insert(0, HistoryEntry { id: time.to_string(), time, key: key.into(), model: model.into(), models: models.to_vec(), provider: prov.clone() });
    list.truncate(HISTORY_MAX);
    all.insert(t.key().into(), serde_json::to_value(&list).map_err(|e| e.to_string())?);
    write_json(&path, &Value::Object(all))
}

#[tauri::command]
pub fn list_history(target: Target) -> R<Vec<HistoryItem>> {
    let all = read_json_object(&history_path()?)?;
    Ok(history_of(&all, target)
        .into_iter()
        .map(|e| HistoryItem { model_count: e.models.len(), key_hint: mask(&e.key), provider: e.provider.name.clone(), host: host_of(&e.provider.claude_base), id: e.id, time: e.time, model: e.model })
        .collect())
}

#[tauri::command]
pub fn apply_history(target: Target, id: String) -> R<String> {
    let all = read_json_object(&history_path()?)?;
    let e = history_of(&all, target)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or("这条导入记录已不存在，请重新打开窗口。")?;
    let name = match target {
        Target::Claude => {
            let x = import_claude(&e.key, &e.model, &e.models, &e.provider)?;
            let _ = record_history(target, &e.key, &e.model, &e.models, &e.provider);
            return Ok(claude_message(&e.model, &x).replacen("已切换到", "已切换回", 1));
        }
        Target::Codex => {
            import_codex(&e.key, &e.model, &e.models, &e.provider)?;
            "Codex"
        }
    };
    let _ = record_history(target, &e.key, &e.model, &e.models, &e.provider);
    Ok(format!("{name} 已切换回 {}（密钥 {}）", e.model, mask(&e.key)))
}

// ---------- status ----------

#[derive(Serialize)]
pub struct ClientStatus {
    target: Target,
    connected: bool,
    /// The client's config folder exists (installed or used before).
    installed: bool,
    model: Option<String>,
    key_hint: Option<String>,
    has_backup: bool,
    config_path: String,
    /// Other entry points configured together (Claude: desktop app, VS Code).
    also: Vec<String>,
    /// Name of the provider the client points at.
    provider: Option<String>,
}

fn mask(key: &str) -> String {
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••{tail}")
}

fn claude_status() -> R<ClientStatus> {
    let path = claude_dir()?.join("settings.json");
    let settings = read_json_object(&path).unwrap_or_default();
    let env = settings.get("env").cloned().unwrap_or(Value::Null);
    let get = |k: &str| env.get(k).and_then(Value::as_str).map(str::to_string);
    let base = get("ANTHROPIC_BASE_URL").map(|u| u.trim_end_matches('/').to_string());
    let connected = base.as_deref().is_some_and(|b| b == CLAUDE_BASE || load_providers().iter().any(|p| p.claude_base == b));
    Ok(ClientStatus {
        target: Target::Claude,
        connected,
        provider: if connected { base.as_deref().map(provider_name) } else { None },
        installed: path.parent().is_some_and(Path::is_dir) || home()?.join(".claude.json").is_file(),
        model: if connected { get("ANTHROPIC_MODEL") } else { None },
        key_hint: if connected { get("ANTHROPIC_AUTH_TOKEN").map(|k| mask(&k)) } else { None },
        has_backup: has_backup(Target::Claude),
        config_path: path.display().to_string(),
        also: [(desktop_installed(), "Claude 桌面版"), (vscode_installed(), "VS Code 插件")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, n)| n.to_string())
            .collect(),
    })
}

fn codex_status() -> R<ClientStatus> {
    let dir = codex_dir()?;
    let path = dir.join("config.toml");
    let doc: toml_edit::DocumentMut = read_opt(&path)?.unwrap_or_default().parse().unwrap_or_default();
    let pid = doc.get("model_provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let connected = pid == PROVIDER_ID || pid == CUSTOM_PROVIDER_ID;
    let pname = doc.get("model_providers").and_then(|m| m.get(&pid)).and_then(|p| p.get("base_url")).and_then(|v| v.as_str()).map(provider_name);
    let auth = read_json_object(&dir.join("auth.json")).unwrap_or_default();
    Ok(ClientStatus {
        target: Target::Codex,
        connected,
        provider: if connected { pname } else { None },
        installed: dir.is_dir(),
        model: if connected { doc.get("model").and_then(|v| v.as_str()).map(str::to_string) } else { None },
        key_hint: if connected {
            auth.get("OPENAI_API_KEY").and_then(Value::as_str).map(mask)
        } else {
            None
        },
        has_backup: has_backup(Target::Codex),
        config_path: path.display().to_string(),
        also: vec![],
    })
}

#[tauri::command]
pub fn status() -> R<Vec<ClientStatus>> {
    Ok(vec![claude_status()?, codex_status()?])
}

/// Tests that point XBTRY_HOME somewhere must hold this, env vars are process-wide.
#[cfg(test)]
pub(crate) static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_parse_and_have_required_fields() {
        for m in ["gpt-6.1-sol", "claude-opus-5-5"] {
            let e = catalog_entry(m, 0).unwrap();
            for k in ["slug", "base_instructions", "supports_reasoning_summaries", "supports_parallel_tool_calls", "shell_type"] {
                assert!(e.get(k).is_some(), "{m} missing {k}");
            }
            assert_eq!(e["slug"], m);
        }
    }

    #[test]
    fn families() {
        assert!(is_gpt_family("gpt-6.1-sol"));
        assert!(is_gpt_family("o4-mini"));
        assert!(!is_gpt_family("claude-opus-5-5"));
        assert!(!is_gpt_family("opus"));
        assert_eq!(pretty_name("gpt-6.1-sol"), "GPT 6.1 Sol");
    }

    #[test]
    fn import_then_restore_roundtrip() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("xbtry-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        std::env::set_var("XBTRY_HOME", &tmp);
        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        let orig_toml = "# mine\nmodel = \"gpt-5\"\n\n[mcp_servers.x]\ncommand = \"x\"\n";
        let orig_claude = "{\"theme\":\"dark\",\"env\":{\"FOO\":\"1\",\"ANTHROPIC_API_KEY\":\"old\"}}";
        fs::write(tmp.join(".codex/config.toml"), orig_toml).unwrap();
        fs::write(tmp.join(".claude/settings.json"), orig_claude).unwrap();
        // Claude desktop + VS Code present inside the sandbox home.
        let (local, roaming) = app_data().unwrap();
        assert!(local.starts_with(&tmp) && roaming.starts_with(&tmp));
        let orig_desk = "{\"mcpServers\":{\"a\":{\"command\":\"a\"}}}";
        fs::create_dir_all(local.join("Claude")).unwrap();
        // A store (MSIX) install as well.
        fs::create_dir_all(local.join("Packages/Claude_test/LocalCache")).unwrap();
        fs::write(local.join("Claude/claude_desktop_config.json"), orig_desk).unwrap();
        let orig_vs = "// mine\n{\n  \"editor.fontSize\": 14, // keep\n}\n";
        fs::create_dir_all(roaming.join("Code/User")).unwrap();
        fs::write(roaming.join("Code/User/settings.json"), orig_vs).unwrap();

        let res = import_config(ImportRequest {
            key: "sk-test1234".into(),
            claude_model: Some("claude-opus-5-5".into()),
            codex_model: Some("claude-opus-5-5".into()),
            models: vec!["gpt-6.1-sol".into(), "claude-opus-5-5".into()],
            provider: None,
        })
        .unwrap();
        assert!(res.iter().all(|r| r.ok), "{:?}", res.iter().map(|r| &r.message).collect::<Vec<_>>());

        let toml = fs::read_to_string(tmp.join(".codex/config.toml")).unwrap();
        let doc: toml_edit::DocumentMut = toml.parse().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("xbtry"));
        assert_eq!(doc["model"].as_str(), Some("claude-opus-5-5"));
        assert_eq!(doc["model_providers"]["xbtry"]["base_url"].as_str(), Some(CODEX_BASE));
        assert!(doc["mcp_servers"]["x"].is_table(), "user sections kept");
        let cat: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".codex").join(CATALOG_FILE)).unwrap()).unwrap();
        assert_eq!(cat["models"].as_array().unwrap().len(), 2);
        assert_eq!(cat["models"][0]["slug"], "claude-opus-5-5");
        // Retry tuning from a fix survives a re-import.
        let tuned = toml.replace("requires_openai_auth = true", "requires_openai_auth = true\nstream_max_retries = 20");
        assert_ne!(tuned, toml);
        fs::write(tmp.join(".codex/config.toml"), &tuned).unwrap();
        import_codex("sk-test1234", "claude-opus-5-5", &["gpt-6.1-sol".into()], &Provider::xbtry()).unwrap();
        let doc2: toml_edit::DocumentMut = fs::read_to_string(tmp.join(".codex/config.toml")).unwrap().parse().unwrap();
        assert_eq!(doc2["model_providers"]["xbtry"]["stream_max_retries"].as_integer(), Some(20));

        let s: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["env"]["FOO"], "1");
        assert_eq!(s["env"]["ANTHROPIC_API_KEY"], "", "inherited OS key neutralized");
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test1234");
        assert_eq!(s["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-opus-5-5");
        assert_eq!(s["model"], "claude-opus-5-5");
        assert_eq!(s["availableModels"][0], "claude-opus-5-5");
        let msg = &res.iter().find(|r| r.target == Target::Claude).unwrap().message;
        assert!(msg.contains("Claude 桌面版") && msg.contains("VS Code"), "{msg}");
        let f = desktop_files().unwrap();
        assert_eq!(f.len(), 8, "normal install + store package");
        let sp: Value = serde_json::from_str(&fs::read_to_string(&f[4]).unwrap()).unwrap();
        assert_eq!(sp["inferenceGatewayBaseUrl"], CLAUDE_BASE, "store package gets the gateway profile");
        assert!(f[4].starts_with(local.join("Packages/Claude_test/LocalCache/Local/Claude-3p")));
        let sm: Value = serde_json::from_str(&fs::read_to_string(&f[7]).unwrap()).unwrap();
        assert_eq!(sm["deploymentMode"], "3p");
        let prof: Value = serde_json::from_str(&fs::read_to_string(&f[0]).unwrap()).unwrap();
        assert_eq!(prof["inferenceGatewayBaseUrl"], CLAUDE_BASE);
        assert_eq!(prof["inferenceGatewayApiKey"], "sk-test1234");
        assert_eq!(prof["inferenceModels"], json!(["claude-opus-5-5"]), "GPT ids filtered for desktop");
        let meta: Value = serde_json::from_str(&fs::read_to_string(&f[1]).unwrap()).unwrap();
        assert_eq!(meta["appliedId"], DESKTOP_PROFILE_ID);
        let desk: Value = serde_json::from_str(&fs::read_to_string(&f[3]).unwrap()).unwrap();
        assert_eq!(desk["deploymentMode"], "3p");
        assert!(desk["mcpServers"]["a"].is_object(), "desktop MCP kept");
        let vs = fs::read_to_string(roaming.join("Code/User/settings.json")).unwrap();
        assert!(vs.contains("// keep") && vs.contains("\"claudeCode.disableLoginPrompt\": true"), "{vs}");
        assert!(!vs.contains("sk-test"), "key never copied into VS Code settings");

        let st = status().unwrap();
        assert!(st.iter().all(|s| s.connected && s.has_backup));

        // Re-import with a new key must keep the original backup.
        import_config(ImportRequest { key: "sk-new".into(), claude_model: Some("x".into()), codex_model: None, models: vec![], provider: None }).unwrap();

        restore_config(Target::Codex).unwrap();
        restore_config(Target::Claude).unwrap();
        assert_eq!(fs::read_to_string(tmp.join(".codex/config.toml")).unwrap(), orig_toml);
        assert!(!tmp.join(".codex/auth.json").exists());
        assert!(!tmp.join(".codex").join(CATALOG_FILE).exists());
        assert!(!tmp.join(".claude.json").exists());
        assert_eq!(fs::read_to_string(tmp.join(".claude/settings.json")).unwrap(), orig_claude);
        assert_eq!(fs::read_to_string(&f[3]).unwrap(), orig_desk);
        assert!(!f[0].exists() && !f[2].exists(), "files XBTry created are removed");
        assert!(f[4..].iter().all(|p| !p.exists()), "store package files removed too");
        assert_eq!(fs::read_to_string(roaming.join("Code/User/settings.json")).unwrap(), orig_vs);
        assert!(restore_config(Target::Codex).is_err());

        // History survives a restore and can re-apply an earlier import.
        let h = list_history(Target::Claude).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].model, "x");
        assert_eq!(h[1].key_hint, "••••1234");
        apply_history(Target::Claude, h[1].id.clone()).unwrap();
        let s: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test1234");
        assert_eq!(s["env"]["ANTHROPIC_MODEL"], "claude-opus-5-5");
        assert_eq!(s["theme"], "dark", "re-apply merges, keeps user settings");
        assert_eq!(list_history(Target::Claude).unwrap()[0].model, "claude-opus-5-5");
        assert!(has_backup(Target::Claude), "re-apply backs up the original first");
        assert_eq!(list_history(Target::Codex).unwrap()[0].model_count, 2);
        assert!(apply_history(Target::Codex, "nope".into()).is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn custom_provider_import() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("xbtry-prov-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        std::env::set_var("XBTRY_HOME", &tmp);
        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        assert!(normalize_provider(Some(Provider { name: "x".into(), claude_base: "ftp://a".into(), codex_base: String::new() })).is_err());
        assert!(normalize_provider(Some(Provider { name: "x".into(), claude_base: "https://u:p@a.com".into(), codex_base: String::new() })).is_err());
        let p = save_provider(Provider { name: " 好中转 ".into(), claude_base: "https://relay.example.com/v1/".into(), codex_base: String::new() }).unwrap();
        assert_eq!(p.claude_base, "https://relay.example.com");
        assert_eq!(p.codex_base, "https://relay.example.com/v1");
        assert_eq!(p.name, "好中转");
        assert_eq!(list_providers().len(), 2);
        let res = import_config(ImportRequest {
            key: "sk-other9999".into(),
            claude_model: Some("claude-opus-5-5".into()),
            codex_model: Some("gpt-6.1-sol".into()),
            models: vec!["gpt-6.1-sol".into()],
            provider: Some(p.clone()),
        })
        .unwrap();
        assert!(res.iter().all(|r| r.ok));
        let doc: toml_edit::DocumentMut = fs::read_to_string(tmp.join(".codex/config.toml")).unwrap().parse().unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some(CUSTOM_PROVIDER_ID));
        assert_eq!(doc["model_providers"][CUSTOM_PROVIDER_ID]["base_url"].as_str(), Some("https://relay.example.com/v1"));
        assert_eq!(doc["model_providers"][CUSTOM_PROVIDER_ID]["name"].as_str(), Some("好中转"));
        let s: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(s["env"]["ANTHROPIC_BASE_URL"], "https://relay.example.com");
        let st = status().unwrap();
        assert!(st.iter().all(|s| s.connected && s.provider.as_deref() == Some("好中转")));
        let h = list_history(Target::Codex).unwrap();
        assert_eq!(h[0].provider, "好中转");
        assert_eq!(h[0].host, "relay.example.com");
        delete_provider(p.claude_base.clone()).unwrap();
        assert_eq!(list_providers().len(), 1);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mask_keeps_tail() {        assert_eq!(mask("sk-abcdef1234"), "••••1234");
    }
}
