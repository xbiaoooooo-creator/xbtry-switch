//! Everything XBTry writes lives here: Claude Code settings, Codex config,
//! the Codex model catalog, and a one-time backup used by "恢复原配置".

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CLAUDE_BASE: &str = "https://api.xbtry.com";
pub const CODEX_BASE: &str = "https://api.xbtry.com/v1";
const PROVIDER_ID: &str = "xbtry";
const CATALOG_FILE: &str = "xbtry-models.json";
const FALLBACK_MODELS: [&str; 3] = ["claude-opus-5-5", "gpt-6.1-sol", "gpt-6-astra"];

const GPT_TEMPLATE: &str = include_str!("../resources/gpt_template.json");
const NATIVE_TEMPLATE: &str = include_str!("../resources/native_template.json");

type R<T> = Result<T, String>;

fn home() -> R<PathBuf> {
    // XBTRY_HOME lets tests (and portable setups) redirect every path.
    if let Some(h) = env_dir("XBTRY_HOME") {
        return Ok(h);
    }
    dirs::home_dir().ok_or_else(|| "找不到当前用户目录".to_string())
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn claude_dir() -> R<PathBuf> {
    Ok(env_dir("CLAUDE_CONFIG_DIR").unwrap_or(home()?.join(".claude")))
}

fn codex_dir() -> R<PathBuf> {
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

fn read_opt(path: &Path) -> R<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败：{e}", path.display())),
    }
}

/// Write through a temp file + rename so a crash never leaves half a config.
fn write_atomic(path: &Path, text: &str) -> R<()> {
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

fn read_json_object(path: &Path) -> R<Map<String, Value>> {
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

fn write_json(path: &Path, value: &Value) -> R<()> {
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

/// Keep the state from *before* XBTry: once a backup exists, re-imports
/// (e.g. a renewed key) never overwrite it.
fn ensure_backup(t: Target) -> R<()> {
    if has_backup(t) {
        return Ok(());
    }
    let dir = backup_dir(t)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let mut entries = Vec::new();
    for (i, path) in t.files()?.into_iter().enumerate() {
        let saved_as = if path.is_file() {
            let name = format!("{i}-{}", path.file_name().unwrap_or_default().to_string_lossy());
            fs::copy(&path, dir.join(&name)).map_err(|e| format!("备份 {} 失败：{e}", path.display()))?;
            Some(name)
        } else {
            None
        };
        entries.push(BackupEntry { path, saved_as });
    }
    let manifest = serde_json::to_value(&entries).map_err(|e| e.to_string())?;
    write_json(&dir.join("manifest.json"), &manifest)
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
pub async fn fetch_models(key: String) -> R<ModelList> {
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
    let res = match client.get(format!("{CODEX_BASE}/models")).bearer_auth(&key).send().await {
        Ok(r) => r,
        Err(_) => return Ok(fallback("网络连接失败，已使用默认模型列表".into())),
    };
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err("API Key 无效或已过期，请检查后重试".into());
    }
    if !status.is_success() {
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

// ---------- import ----------

#[derive(Deserialize)]
pub struct ImportRequest {
    key: String,
    claude_model: Option<String>,
    codex_model: Option<String>,
    /// Every model shown in the app; all become selectable inside Codex.
    #[serde(default)]
    models: Vec<String>,
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

fn import_claude(key: &str, model: &str) -> R<()> {
    let settings_path = claude_dir()?.join("settings.json");
    let mut settings = read_json_object(&settings_path)?;
    let state_path = home()?.join(".claude.json");
    let mut state = read_json_object(&state_path)?;
    ensure_backup(Target::Claude)?;

    let mut env = match settings.remove("env") {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    // Two auth variables at once make Claude Code warn and pick one at random.
    env.remove("ANTHROPIC_API_KEY");
    env.insert("ANTHROPIC_BASE_URL".into(), json!(CLAUDE_BASE));
    env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(key));
    // All aliases point at the chosen model, so a GPT model works too and
    // Claude Code's background haiku/sonnet calls never ask for a missing name.
    for k in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL",
    ] {
        env.insert(k.into(), json!(model));
    }
    settings.insert("env".into(), Value::Object(env));
    write_json(&settings_path, &Value::Object(settings))?;

    // Skip the first-run login screen; the token above is the login.
    state.insert("hasCompletedOnboarding".into(), json!(true));
    write_json(&state_path, &Value::Object(state))
}

fn import_codex(key: &str, model: &str, models: &[String]) -> R<()> {
    let dir = codex_dir()?;
    let config_path = dir.join("config.toml");
    let text = read_opt(&config_path)?.unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .trim_start_matches('\u{feff}')
        .parse()
        .map_err(|_| format!("{} 格式有误，为避免覆盖你的配置已停止。", config_path.display()))?;
    ensure_backup(Target::Codex)?;

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

    doc["model_provider"] = toml_edit::value(PROVIDER_ID);
    doc["model"] = toml_edit::value(model);
    doc["model_catalog_json"] = toml_edit::value(catalog_path.to_string_lossy().to_string());
    if doc.get("model_providers").and_then(|i| i.as_table()).is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc["model_providers"] = toml_edit::Item::Table(t);
    }
    let mut provider = toml_edit::Table::new();
    provider["name"] = toml_edit::value("XBTry");
    provider["base_url"] = toml_edit::value(CODEX_BASE);
    provider["wire_api"] = toml_edit::value("responses");
    provider["requires_openai_auth"] = toml_edit::value(true);
    doc["model_providers"][PROVIDER_ID] = toml_edit::Item::Table(provider);
    write_atomic(&config_path, &doc.to_string())?;

    write_json(&dir.join("auth.json"), &json!({ "OPENAI_API_KEY": key }))
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
    let mut out = Vec::new();
    if let Some(m) = clean(&req.claude_model) {
        let r = import_claude(key, &m);
        out.push(ImportResult {
            target: Target::Claude,
            ok: r.is_ok(),
            message: r.err().unwrap_or_else(|| format!("Claude Code 已切换到 {m}")),
        });
    }
    if let Some(m) = clean(&req.codex_model) {
        let r = import_codex(key, &m, &req.models);
        out.push(ImportResult {
            target: Target::Codex,
            ok: r.is_ok(),
            message: r.err().unwrap_or_else(|| format!("Codex 已切换到 {m}")),
        });
    }
    if out.is_empty() {
        return Err("请至少选择一个客户端".into());
    }
    Ok(out)
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
    let connected = get("ANTHROPIC_BASE_URL").is_some_and(|u| u.trim_end_matches('/') == CLAUDE_BASE);
    Ok(ClientStatus {
        target: Target::Claude,
        connected,
        installed: path.parent().is_some_and(Path::is_dir) || home()?.join(".claude.json").is_file(),
        model: if connected { get("ANTHROPIC_MODEL") } else { None },
        key_hint: if connected { get("ANTHROPIC_AUTH_TOKEN").map(|k| mask(&k)) } else { None },
        has_backup: has_backup(Target::Claude),
        config_path: path.display().to_string(),
    })
}

fn codex_status() -> R<ClientStatus> {
    let dir = codex_dir()?;
    let path = dir.join("config.toml");
    let doc: toml_edit::DocumentMut = read_opt(&path)?.unwrap_or_default().parse().unwrap_or_default();
    let connected = doc.get("model_provider").and_then(|v| v.as_str()) == Some(PROVIDER_ID);
    let auth = read_json_object(&dir.join("auth.json")).unwrap_or_default();
    Ok(ClientStatus {
        target: Target::Codex,
        connected,
        installed: dir.is_dir(),
        model: if connected { doc.get("model").and_then(|v| v.as_str()).map(str::to_string) } else { None },
        key_hint: if connected {
            auth.get("OPENAI_API_KEY").and_then(Value::as_str).map(mask)
        } else {
            None
        },
        has_backup: has_backup(Target::Codex),
        config_path: path.display().to_string(),
    })
}

#[tauri::command]
pub fn status() -> R<Vec<ClientStatus>> {
    Ok(vec![claude_status()?, codex_status()?])
}

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

        let res = import_config(ImportRequest {
            key: "sk-test1234".into(),
            claude_model: Some("gpt-6.1-sol".into()),
            codex_model: Some("claude-opus-5-5".into()),
            models: vec!["gpt-6.1-sol".into(), "claude-opus-5-5".into()],
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

        let s: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(s["theme"], "dark");
        assert_eq!(s["env"]["FOO"], "1");
        assert!(s["env"].get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test1234");
        assert_eq!(s["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "gpt-6.1-sol");

        let st = status().unwrap();
        assert!(st.iter().all(|s| s.connected && s.has_backup));

        // Re-import with a new key must keep the original backup.
        import_config(ImportRequest { key: "sk-new".into(), claude_model: Some("x".into()), codex_model: None, models: vec![] }).unwrap();

        restore_config(Target::Codex).unwrap();
        restore_config(Target::Claude).unwrap();
        assert_eq!(fs::read_to_string(tmp.join(".codex/config.toml")).unwrap(), orig_toml);
        assert!(!tmp.join(".codex/auth.json").exists());
        assert!(!tmp.join(".codex").join(CATALOG_FILE).exists());
        assert!(!tmp.join(".claude.json").exists());
        assert_eq!(fs::read_to_string(tmp.join(".claude/settings.json")).unwrap(), orig_claude);
        assert!(restore_config(Target::Codex).is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn mask_keeps_tail() {        assert_eq!(mask("sk-abcdef1234"), "••••1234");
    }
}
