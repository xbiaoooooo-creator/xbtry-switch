//! XBTry 2.2: one-click checks, verified one-click fixes (backed up and
//! undoable), error-text diagnosis, proxy detection and install helpers.
//!
//! Fixes only touch the keys they need. Each fix stores the files' previous
//! contents under ~/.xbtry/backup/fixes/<id>/ plus the old value of every key
//! it changed, kept apart from the import backup. Undo puts the whole file back
//! when nothing else changed it since, otherwise only the keys this fix set.

use crate::config::{claude_dir, codex_dir, env_dir, home, read_json_object, read_opt, write_atomic, R, CLAUDE_BASE, CODEX_BASE, PROVIDER_ID};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use toml_edit::{DocumentMut, Item, Table, TableLike};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// Local HTTP proxy ports of common proxy apps (Clash Verge, Clash, v2rayN,
/// V2rayU, Surge, Shadowsocks, Qv2ray). Socks-only ports are left out.
const PROXY_PORTS: [u16; 9] = [7897, 7890, 10809, 1087, 6152, 1081, 8889, 20171, 8080];
const PROXY_KEYS: [&str; 2] = ["HTTP_PROXY", "HTTPS_PROXY"];

// ---------- results ----------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Info,
    Warn,
    Err,
}

#[derive(Serialize, Clone, Debug)]
pub struct FixOffer {
    id: String,
    label: String,
    params: Value,
}

fn offer(id: &str, label: &str, params: Value) -> FixOffer {
    FixOffer { id: id.into(), label: label.into(), params }
}

#[derive(Serialize, Debug)]
pub struct Check {
    group: &'static str,
    title: String,
    level: Level,
    detail: String,
    fix: Option<FixOffer>,
    /// Entry card that helps with this item ("import", "proxy", "install", "version").
    open: Option<&'static str>,
}

fn check(group: &'static str, title: impl Into<String>, level: Level, detail: impl Into<String>) -> Check {
    Check { group, title: title.into(), level, detail: detail.into(), fix: None, open: None }
}

impl Check {
    fn fix(mut self, f: FixOffer) -> Self {
        self.fix = Some(f);
        self
    }
    fn open(mut self, card: &'static str) -> Self {
        self.open = Some(card);
        self
    }
}

// ---------- processes and tools ----------

fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Run a program with a hard timeout; returns stdout (or stderr when stdout is empty).
fn run_capture(prog: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = quiet(&mut cmd).spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(if text.is_empty() { String::from_utf8_lossy(&out.stderr).trim().to_string() } else { text })
}

fn version_of(p: &Path) -> Option<String> {
    let out = run_capture(p, &["--version"], Duration::from_secs(8))?;
    let line = out.lines().next().unwrap_or("").trim().to_string();
    line.split_whitespace()
        .map(|t| t.trim_start_matches('v'))
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))
        .map(str::to_string)
        .or_else(|| (!line.is_empty()).then_some(line))
}

fn exe_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"].iter().map(|e| format!("{name}.{e}")).collect()
    } else {
        vec![name.to_string()]
    }
}

#[cfg(windows)]
fn expand_env(s: &str) -> String {
    let (mut out, mut rest) = (String::new(), s);
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                out.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out + rest
}

/// PATH as the app saw it at launch, plus the registry PATH so tools installed
/// while XBTry is open are found too.
fn path_dirs() -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut v: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    #[cfg(windows)]
    {
        use winreg::{enums::*, RegKey};
        for (root, sub) in [(HKEY_CURRENT_USER, "Environment"), (HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")] {
            if let Ok(s) = RegKey::predef(root).open_subkey(sub).and_then(|k| k.get_value::<String, _>("Path")) {
                v.extend(s.split(';').map(str::trim).filter(|x| !x.is_empty()).map(|x| PathBuf::from(expand_env(x))));
            }
        }
    }
    v
}

/// Where installers put tools when they are not on PATH (GUI apps on macOS get a minimal PATH).
fn common_dirs() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(h) = dirs::home_dir() {
        v.push(h.join(".local").join("bin"));
        v.push(h.join(".npm-global").join("bin"));
        v.push(h.join(".claude").join("local"));
    }
    #[cfg(windows)]
    {
        if let Some(a) = env_dir("APPDATA") {
            v.push(a.join("npm"));
        }
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = env_dir(pf) {
                v.push(p.join("Git").join("cmd"));
                v.push(p.join("nodejs"));
            }
        }
        if let Some(l) = env_dir("LOCALAPPDATA") {
            v.push(l.join("Programs").join("Git").join("cmd"));
        }
    }
    #[cfg(not(windows))]
    {
        v.push("/opt/homebrew/bin".into());
        v.push("/usr/local/bin".into());
    }
    v
}

fn find_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names = exe_names(name);
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let mut dirs = path_dirs();
    dirs.extend(common_dirs());
    find_in(name, &dirs)
}

/// The Codex desktop app ships its own CLI under a versioned folder.
fn codex_desktop_bin() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let root = env_dir("LOCALAPPDATA")?.join("OpenAI").join("Codex").join("bin");
        let mut found: Vec<(SystemTime, PathBuf)> = fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|e| e.path().join("codex.exe"))
            .filter(|p| p.is_file())
            .map(|p| (fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), p))
            .collect();
        found.sort();
        return found.pop().map(|(_, p)| p);
    }
    #[cfg(target_os = "macos")]
    {
        let p = PathBuf::from("/Applications/Codex.app/Contents/Resources/codex");
        return p.is_file().then_some(p);
    }
    #[allow(unreachable_code)]
    None
}

/// Same lookup Claude Code does: git on PATH -> <git>/bin/bash.exe, then the default install dirs.
fn git_bash() -> Option<PathBuf> {
    let mut c = vec![];
    if let Some(g) = find_tool("git") {
        if let Some(root) = g.parent().and_then(Path::parent) {
            c.push(root.join("bin").join("bash.exe"));
        }
    }
    for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(p) = env_dir(pf) {
            c.push(p.join("Git").join("bin").join("bash.exe"));
        }
    }
    if let Some(l) = env_dir("LOCALAPPDATA") {
        c.push(l.join("Programs").join("Git").join("bin").join("bash.exe"));
    }
    c.into_iter().find(|p| p.is_file())
}
// ---------- proxy ----------

#[derive(Serialize, Debug)]
pub struct ProxyInfo {
    system: Option<String>,
    system_on: bool,
    /// Local proxy addresses that accept connections right now.
    listening: Vec<String>,
    codex: Option<String>,
    claude: Option<String>,
    suggested: Option<String>,
}

#[cfg(windows)]
pub(crate) fn system_proxy() -> (Option<String>, bool) {
    use winreg::{enums::*, RegKey};
    let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings") else {
        return (None, false);
    };
    let on = k.get_value::<u32, _>("ProxyEnable").unwrap_or(0) == 1;
    let raw: String = k.get_value("ProxyServer").unwrap_or_default();
    let server = if raw.contains('=') {
        raw.split(';').map(str::trim).find_map(|p| p.strip_prefix("https=").or_else(|| p.strip_prefix("http=")))
    } else {
        Some(raw.trim())
    };
    let server = server.filter(|s| !s.is_empty()).map(|s| if s.contains("://") { s.to_string() } else { format!("http://{s}") });
    (server, on)
}

#[cfg(target_os = "macos")]
pub(crate) fn system_proxy() -> (Option<String>, bool) {
    let Some(out) = run_capture(Path::new("/usr/sbin/scutil"), &["--proxy"], Duration::from_secs(3)) else {
        return (None, false);
    };
    let get = |k: &str| {
        out.lines().find_map(|l| {
            let (a, b) = l.split_once(':')?;
            (a.trim() == k).then(|| b.trim().to_string())
        })
    };
    let on = get("HTTPSEnable").as_deref() == Some("1") || get("HTTPEnable").as_deref() == Some("1");
    let host = get("HTTPSProxy").or_else(|| get("HTTPProxy"));
    let port = get("HTTPSPort").or_else(|| get("HTTPPort"));
    (host.zip(port).map(|(h, p)| format!("http://{h}:{p}")), on)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn system_proxy() -> (Option<String>, bool) {
    (None, false)
}

/// Port of a proxy URL that points at this machine.
fn local_port(url: &str) -> Option<u16> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r).trim_end_matches('/');
    let (host, port) = rest.rsplit_once(':')?;
    matches!(host, "127.0.0.1" | "localhost" | "[::1]").then(|| port.parse().ok()).flatten()
}

fn listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)).is_ok()
}

fn probe_ports(ports: &[u16]) -> Vec<u16> {
    std::thread::scope(|s| {
        let hs: Vec<_> = ports.iter().map(|&p| s.spawn(move || listening(p).then_some(p))).collect();
        hs.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    })
}

/// Normalize a proxy address; only scheme://host:port, no credentials.
fn valid_proxy(url: &str) -> R<String> {
    let u = url.trim().trim_end_matches('/');
    let (scheme, rest) = ["http://", "https://", "socks5://", "socks5h://"]
        .iter()
        .find_map(|s| u.strip_prefix(s).map(|r| (*s, r)))
        .ok_or("代理地址要以 http://、https:// 或 socks5:// 开头，例如 http://127.0.0.1:7897")?;
    if rest.contains('@') {
        return Err("XBTry 只写入不带账号密码的代理地址".into());
    }
    let (host, port) = rest.rsplit_once(':').ok_or("代理地址缺少端口，例如 http://127.0.0.1:7897")?;
    let port_ok = port.parse::<u16>().is_ok_and(|p| p > 0);
    let host_ok = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || ".-[]:".contains(c));
    if !port_ok || !host_ok {
        return Err("代理地址格式不对，例如 http://127.0.0.1:7897".into());
    }
    Ok(format!("{scheme}{host}:{port}"))
}

fn codex_env_path() -> R<PathBuf> {
    Ok(codex_dir()?.join(".env"))
}

fn claude_settings_path() -> R<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

fn codex_toml_path() -> R<PathBuf> {
    Ok(codex_dir()?.join("config.toml"))
}

fn claude_env(key: &str) -> Option<String> {
    let s = read_json_object(&claude_settings_path().ok()?).ok()?;
    s.get("env")?.get(key)?.as_str().filter(|v| !v.is_empty()).map(str::to_string)
}

fn codex_env(key: &str) -> Option<String> {
    env_get(&read_opt(&codex_env_path().ok()?).ok()??, key).filter(|v| !v.is_empty())
}

fn proxy_info() -> ProxyInfo {
    let (system, system_on) = system_proxy();
    let codex = codex_env("HTTPS_PROXY").or_else(|| codex_env("HTTP_PROXY"));
    let claude = claude_env("HTTPS_PROXY").or_else(|| claude_env("HTTP_PROXY"));
    let mut ports: Vec<u16> = vec![];
    for p in [&system, &codex, &claude].into_iter().flatten().filter_map(|u| local_port(u)).chain(PROXY_PORTS) {
        if !ports.contains(&p) {
            ports.push(p);
        }
    }
    let open = probe_ports(&ports);
    let listening: Vec<String> = ports.iter().filter(|p| open.contains(p)).map(|p| format!("http://127.0.0.1:{p}")).collect();
    let suggested = system
        .as_ref()
        .filter(|s| local_port(s).is_some_and(|p| open.contains(&p)))
        .cloned()
        .or_else(|| listening.first().cloned());
    ProxyInfo { system, system_on, listening, codex, claude, suggested }
}

#[tauri::command]
pub async fn detect_proxy() -> R<ProxyInfo> {
    tauri::async_runtime::spawn_blocking(proxy_info).await.map_err(|e| e.to_string())
}

// ---------- .env helpers ----------

fn env_get(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let l = l.strip_prefix("export ").unwrap_or(l);
        let (k, v) = l.split_once('=')?;
        (!l.starts_with('#') && k.trim() == key).then(|| v.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

/// Set or remove one KEY=VALUE line, keeping comments and other lines.
fn env_put(text: &str, key: &str, val: Option<&str>) -> String {
    let mut out: Vec<String> = vec![];
    let mut done = false;
    for line in text.lines() {
        let t = line.trim();
        let t2 = t.strip_prefix("export ").unwrap_or(t);
        let hit = !t.starts_with('#') && t2.split_once('=').is_some_and(|(k, _)| k.trim() == key);
        if hit {
            if !done {
                if let Some(v) = val {
                    out.push(format!("{key}={v}"));
                }
            }
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if let Some(v) = val {
            out.push(format!("{key}={v}"));
        }
    }
    if out.is_empty() { String::new() } else { out.join("\n") + "\n" }
}
// ---------- marked blocks (AGENTS.md) ----------

fn block_marks(name: &str) -> (String, String) {
    (format!("<!-- xbtry:{name} -->"), format!("<!-- /xbtry:{name} -->"))
}

fn block_get(text: &str, name: &str) -> Option<String> {
    let (b, e) = block_marks(name);
    let s = text.find(&b)? + b.len();
    let end = s + text[s..].find(&e)?;
    Some(text[s..end].trim_matches('\n').to_string())
}

/// Add, replace or remove XBTry's marked block; the rest of the file is untouched.
fn block_put(text: &str, name: &str, val: Option<&str>) -> String {
    let (b, e) = block_marks(name);
    let mut out = text.to_string();
    if let Some(s) = text.find(&b) {
        if let Some(rel) = text[s..].find(&e) {
            let mut end = s + rel + e.len();
            if out[end..].starts_with('\n') {
                end += 1;
            }
            let start = if out[..s].ends_with("\n\n") { s - 1 } else { s };
            out.replace_range(start..end, "");
        }
    }
    if let Some(v) = val {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out += &format!("{b}\n{v}\n{e}\n");
    }
    out
}

// ---------- fix transactions ----------

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Env,
    Toml,
    Json,
    /// XBTry's own marked block in a text file (AGENTS.md).
    Block,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Change {
    file: PathBuf,
    kind: Kind,
    keys: Vec<String>,
    /// Previous value; TOML values are kept as their source text.
    old: Option<Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct SavedFile {
    path: PathBuf,
    /// Copy inside the record folder; `None` means the file did not exist.
    before: Option<String>,
    /// Fingerprint of the content XBTry wrote (no file contents in the manifest).
    after: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FixRecord {
    id: String,
    fix: String,
    title: String,
    time: u64,
    files: Vec<SavedFile>,
    changes: Vec<Change>,
    /// Folder whose permissions were reset (icacls backup acl.txt in the record folder).
    #[serde(default)]
    acl: Option<String>,
}

#[derive(Default)]
struct Tx {
    /// (path, content before, content after)
    files: Vec<(PathBuf, Option<String>, String)>,
    changes: Vec<Change>,
}

fn fingerprint(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}-{}", h.finish(), s.len())
}

fn toml_repr(v: &toml_edit::Value) -> String {
    let mut v = v.clone();
    v.decor_mut().clear();
    v.to_string()
}

fn parse_toml(text: Option<&str>, path: &Path) -> R<DocumentMut> {
    text.unwrap_or("")
        .trim_start_matches('\u{feff}')
        .parse::<DocumentMut>()
        .map_err(|_| format!("{} 格式有误，为避免覆盖你的配置已停止。", path.display()))
}

fn parse_json(text: Option<&str>, path: &Path) -> R<Map<String, Value>> {
    let t = text.unwrap_or("").trim_start_matches('\u{feff}');
    if t.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(t) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(format!("{} 不是有效的 JSON，为避免覆盖你的配置已停止。", path.display())),
    }
}

/// Set or remove a nested TOML key; returns the old value's source text.
/// Removing the last key of a table also drops the now-empty table.
fn toml_write(doc: &mut DocumentMut, keys: &[&str], val: Option<toml_edit::Value>) -> Option<String> {
    let (last, parents) = keys.split_last()?;
    let removing = val.is_none();
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for k in parents {
        if table.get(k).is_none() {
            if removing {
                return None;
            }
            table.insert(k, Item::Table(Table::new()));
        }
        table = table.get_mut(k)?.as_table_like_mut()?;
    }
    let old = table.get(last).and_then(Item::as_value).map(toml_repr);
    match val {
        Some(v) => {
            table.insert(last, Item::Value(v));
        }
        None => {
            table.remove(last);
        }
    }
    let empty = table.is_empty();
    if removing && parents.len() == 1 && empty {
        doc.as_table_mut().remove(parents[0]);
    }
    old
}

fn json_write(obj: &mut Map<String, Value>, keys: &[&str], val: Option<Value>) -> R<Option<Value>> {
    let Some((last, parents)) = keys.split_last() else { return Ok(None) };
    let removing = val.is_none();
    let mut cur = &mut *obj;
    for k in parents {
        if !cur.contains_key(*k) {
            if removing {
                return Ok(None);
            }
            cur.insert(k.to_string(), json!({}));
        }
        cur = cur.get_mut(*k).and_then(Value::as_object_mut).ok_or_else(|| format!("配置里的 {k} 不是对象，未修改"))?;
    }
    let old = match val {
        Some(v) => cur.insert(last.to_string(), v),
        None => cur.shift_remove(*last),
    };
    let empty = cur.is_empty();
    if removing && parents.len() == 1 && empty {
        obj.shift_remove(parents[0]);
    }
    Ok(old)
}

impl Tx {
    fn text(&self, p: &Path) -> R<Option<String>> {
        match self.files.iter().find(|f| f.0 == p) {
            Some(f) => Ok(Some(f.2.clone())),
            None => read_opt(p),
        }
    }

    fn put(&mut self, p: &Path, text: String) -> R<()> {
        if let Some(f) = self.files.iter_mut().find(|f| f.0 == p) {
            f.2 = text;
        } else {
            let before = read_opt(p)?;
            self.files.push((p.to_path_buf(), before, text));
        }
        Ok(())
    }

    fn record(&mut self, file: &Path, kind: Kind, keys: &[&str], old: Option<Value>) {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        // A key touched twice in one fix keeps its very first old value.
        if !self.changes.iter().any(|c| c.file == file && c.keys == keys) {
            self.changes.push(Change { file: file.to_path_buf(), kind, keys, old });
        }
    }

    fn env_set(&mut self, p: &Path, key: &str, val: Option<&str>) -> R<()> {
        let text = self.text(p)?.unwrap_or_default();
        let old = env_get(&text, key);
        if old.as_deref() == val {
            return Ok(());
        }
        self.put(p, env_put(&text, key, val))?;
        self.record(p, Kind::Env, &[key], old.map(Value::String));
        Ok(())
    }

    fn json_set(&mut self, p: &Path, keys: &[&str], val: Option<Value>) -> R<()> {
        let text = self.text(p)?;
        let mut obj = parse_json(text.as_deref(), p)?;
        let old = json_write(&mut obj, keys, val.clone())?;
        if old == val {
            return Ok(());
        }
        let out = serde_json::to_string_pretty(&Value::Object(obj)).map_err(|e| e.to_string())? + "\n";
        self.put(p, out)?;
        self.record(p, Kind::Json, keys, old);
        Ok(())
    }

    fn toml_set(&mut self, p: &Path, keys: &[&str], val: Option<toml_edit::Value>) -> R<()> {
        let text = self.text(p)?;
        let mut doc = parse_toml(text.as_deref(), p)?;
        let new = val.as_ref().map(toml_repr);
        let old = toml_write(&mut doc, keys, val);
        if old == new {
            return Ok(());
        }
        self.put(p, doc.to_string())?;
        self.record(p, Kind::Toml, keys, old.map(Value::String));
        Ok(())
    }

    fn block_set(&mut self, p: &Path, name: &str, val: Option<&str>) -> R<()> {
        let text = self.text(p)?.unwrap_or_default();
        let old = block_get(&text, name);
        if old.as_deref() == val {
            return Ok(());
        }
        self.put(p, block_put(&text, name, val))?;
        self.record(p, Kind::Block, &[name], old.map(Value::String));
        Ok(())
    }

    /// Put one recorded key back to its old value.
    fn revert(&mut self, c: &Change) -> R<()> {
        let keys: Vec<&str> = c.keys.iter().map(String::as_str).collect();
        match c.kind {
            Kind::Env => self.env_set(&c.file, keys[0], c.old.as_ref().and_then(Value::as_str)),
            Kind::Json => self.json_set(&c.file, &keys, c.old.clone()),
            Kind::Block => self.block_set(&c.file, keys[0], c.old.as_ref().and_then(Value::as_str)),
            Kind::Toml => {
                let v = match c.old.as_ref().and_then(Value::as_str) {
                    Some(s) => Some(s.parse::<toml_edit::Value>().map_err(|_| "备份记录已损坏，未做任何修改。".to_string())?),
                    None => None,
                };
                self.toml_set(&c.file, &keys, v)
            }
        }
    }
}

fn fixes_dir() -> R<PathBuf> {
    Ok(home()?.join(".xbtry").join("backup").join("fixes"))
}

fn load_records() -> R<Vec<FixRecord>> {
    match read_opt(&fixes_dir()?.join("manifest.json"))? {
        None => Ok(vec![]),
        Some(t) => serde_json::from_str(&t).map_err(|_| "修复记录已损坏，未做任何修改。".to_string()),
    }
}

fn save_records(list: &[FixRecord]) -> R<()> {
    let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
    write_atomic(&fixes_dir()?.join("manifest.json"), &(text + "\n"))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn commit(fix: &str, title: String, tx: Tx) -> R<String> {
    if tx.changes.is_empty() {
        return Ok(format!("{title}：已经是这个设置，无需修改"));
    }
    let mut list = load_records()?;
    let mut time = now_ms();
    while list.iter().any(|r| r.time == time) {
        time += 1;
    }
    let id = format!("{time}-{fix}");
    let dir = fixes_dir()?.join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let mut files = vec![];
    for (i, (path, before, after)) in tx.files.iter().enumerate() {
        let before = match before {
            Some(b) => {
                let name = format!("{i}-{}", path.file_name().unwrap_or_default().to_string_lossy());
                fs::write(dir.join(&name), b).map_err(|e| format!("备份 {} 失败：{e}", path.display()))?;
                Some(name)
            }
            None => None,
        };
        files.push(SavedFile { path: path.clone(), before, after: fingerprint(after) });
    }
    list.insert(0, FixRecord { id: id.clone(), fix: fix.into(), title: title.clone(), time, files, changes: tx.changes, acl: None });
    save_records(&list)?;
    for (i, (path, _, after)) in tx.files.iter().enumerate() {
        if let Err(e) = write_atomic(path, after) {
            // Roll back what was already written, then drop the record.
            for (p, before, _) in &tx.files[..i] {
                let _ = match before {
                    Some(b) => write_atomic(p, b),
                    None => fs::remove_file(p).map_err(|e| e.to_string()),
                };
            }
            list.remove(0);
            let _ = save_records(&list);
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
    }
    Ok(format!("{title}：已完成"))
}

#[derive(Deserialize, Default, Debug)]
pub struct FixParams {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

fn mcp_timeouts(path: &Path) -> R<Vec<(String, Option<f64>)>> {
    let doc = parse_toml(read_opt(path)?.as_deref(), path)?;
    Ok(doc
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .map(|t| {
            t.iter()
                .filter(|(_, v)| v.is_table_like())
                .map(|(k, v)| {
                    let n = v.get("startup_timeout_sec").and_then(|x| x.as_integer().map(|i| i as f64).or_else(|| x.as_float()));
                    (k.to_string(), n)
                })
                .collect()
        })
        .unwrap_or_default())
}

const MCP_STARTUP_SEC: i64 = 60;
/// Codex defaults: 5 stream reconnects ("Reconnecting... 5/5"), 4 request retries; hard cap 100.
const STREAM_RETRIES: i64 = 20;
const REQUEST_RETRIES: i64 = 10;
const OPENAI_RETRY_MSG: &str = "Codex 现在用的是官方 OpenAI 登录，Codex 不允许修改内置 provider 的重连次数。断线多的话，请在「代理设置」里让 Codex 走本机代理，或用「API 导入」接入 XBTry 后再调重连次数";
const ZH_CODEX: &str = "## 回复语言\n\n始终使用简体中文回复，包括说明、计划、总结和提问。代码、命令、文件路径、报错原文和专有名词保持原样。";
const ZH_CLAUDE: &str = "Simplified Chinese";

/// The Codex provider XBTry may tune: a user-defined one. Codex rejects overrides of built-in `openai`.
fn custom_provider(doc: &DocumentMut) -> Option<String> {
    let p = doc.get("model_provider")?.as_str()?;
    (p != "openai" && doc.get("model_providers")?.get(p)?.is_table_like()).then(|| p.to_string())
}

fn retries_of(doc: &DocumentMut, p: &str, k: &str) -> Option<i64> {
    doc.get("model_providers")?.get(p)?.get(k)?.as_integer()
}

/// Codex reads AGENTS.override.md instead of AGENTS.md when it is non-empty.
fn codex_agents_path() -> R<PathBuf> {
    let d = codex_dir()?;
    let o = d.join("AGENTS.override.md");
    Ok(if read_opt(&o)?.is_some_and(|t| !t.trim().is_empty()) { o } else { d.join("AGENTS.md") })
}

// ---------- Chinese UI ----------
/// Codex desktop: official `localeOverride` setting, stored in config.toml [desktop].
const CODEX_UI_LOCALE: &str = "zh-CN";
/// Claude desktop: `locale` in its config.json; the app ships zh-Hans.json.
const CLAUDE_UI_LOCALE: &str = "zh-Hans";
/// Gateway (API) mode loads the bundled UI, which has no Chinese catalog.
pub(crate) const CLAUDE_GATEWAY_ZH: &str = "Claude 官方暂未给 API 接入模式提供中文界面，已为你设置中文回复。需要中文界面的话，推荐用 Codex 接入 XBTry。";

/// Claude desktop runs through a gateway (deploymentMode = 3p) in any folder it may read.
pub(crate) fn claude_gateway_mode() -> bool {
    claude_desktop_configs().iter().filter_map(|p| p.parent()).any(|d| {
        read_json_object(&d.join("claude_desktop_config.json"))
            .ok()
            .and_then(|m| m.get("deploymentMode").and_then(Value::as_str).map(|s| s == "3p"))
            .unwrap_or(false)
    })
}

fn local_roaming() -> Option<(PathBuf, PathBuf)> {
    let h = home().ok()?;
    if env_dir("XBTRY_HOME").is_some() || !cfg!(windows) {
        return Some((h.join("AppData").join("Local"), h.join("AppData").join("Roaming")));
    }
    Some((env_dir("LOCALAPPDATA")?, env_dir("APPDATA")?))
}

/// Claude desktop config.json files. Store (MSIX) installs keep it under
/// Packages\<family>\LocalCache\Roaming\Claude. Files in use come first.
fn claude_desktop_configs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![];
    if cfg!(target_os = "macos") && env_dir("XBTRY_HOME").is_none() {
        if let Some(h) = dirs::home_dir() {
            dirs.push(h.join("Library").join("Application Support").join("Claude"));
            dirs.push(h.join("Library").join("Application Support").join("Claude-3p"));
        }
    } else if let Some((local, roaming)) = local_roaming() {
        dirs.push(roaming.join("Claude"));
        // Gateway (3p) mode keeps its own data folder, with its own config.json.
        dirs.push(local.join("Claude-3p"));
        if let Ok(rd) = fs::read_dir(local.join("Packages")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("Claude_") || n.starts_with("AnthropicPBC.Claude_") {
                    dirs.push(e.path().join("LocalCache").join("Roaming").join("Claude"));
                    dirs.push(e.path().join("LocalCache").join("Local").join("Claude-3p"));
                }
            }
        }
    }
    let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| d.is_dir()).collect();
    // A folder is in use when it has config.json or a desktop config (set up / gateway mode).
    let used: Vec<PathBuf> = dirs
        .iter()
        .filter(|d| d.join("config.json").is_file() || d.join("claude_desktop_config.json").is_file())
        .map(|d| d.join("config.json"))
        .collect();
    if used.is_empty() { dirs.into_iter().map(|d| d.join("config.json")).collect() } else { used }
}

/// Claude desktop rewrites config.json when it quits, so it must be closed first.
fn claude_desktop_running() -> bool {
    if env_dir("XBTRY_HOME").is_some() {
        return false;
    }
    #[cfg(windows)]
    {
        let out = run_capture(
            Path::new("powershell.exe"),
            &["-NoProfile", "-Command", "(Get-Process -Name claude -ErrorAction SilentlyContinue).Path"],
            Duration::from_secs(8),
        )
        .unwrap_or_default()
        .to_ascii_lowercase();
        // The Claude Code CLI is also claude.exe; only count the desktop app.
        return out.lines().any(|l| l.contains("windowsapps") || l.contains("anthropicclaude") || l.contains("\\programs\\claude"));
    }
    #[cfg(target_os = "macos")]
    {
        return run_capture(Path::new("/usr/bin/pgrep"), &["-x", "Claude"], Duration::from_secs(3)).is_some_and(|s| !s.trim().is_empty());
    }
    #[allow(unreachable_code)]
    false
}

fn claude_ui_zh(tx: &mut Tx) -> R<()> {
    let files = claude_desktop_configs();
    if files.is_empty() {
        return Err("没有检测到 Claude 桌面版".into());
    }
    if claude_desktop_running() {
        return Err("Claude 桌面版正在运行，请先从托盘完全退出再试（否则它退出时会把设置改回去）".into());
    }
    for f in files {
        tx.json_set(&f, &["locale"], Some(json!(CLAUDE_UI_LOCALE)))?;
    }
    Ok(())
}

#[tauri::command]
pub fn apply_fix(id: String, params: Option<FixParams>) -> R<String> {
    if id == "codex_acl_reset" {
        return acl_reset();
    }
    let p = params.unwrap_or_default();
    let codex_env = codex_env_path()?;
    let codex_toml = codex_toml_path()?;
    let claude = claude_settings_path()?;
    let s = |v: &str| Some(json!(v));
    let mut tx = Tx::default();
    let title = match id.as_str() {
        "codex_proxy" => {
            let u = valid_proxy(p.url.as_deref().unwrap_or(""))?;
            for k in PROXY_KEYS {
                tx.env_set(&codex_env, k, Some(&u))?;
            }
            format!("Codex 代理设为 {u}")
        }
        "claude_proxy" => {
            let u = valid_proxy(p.url.as_deref().unwrap_or(""))?;
            for k in PROXY_KEYS {
                tx.json_set(&claude, &["env", k], s(&u))?;
            }
            format!("Claude Code 代理设为 {u}")
        }
        "proxy_clear" => {
            for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"] {
                tx.env_set(&codex_env, k, None)?;
                tx.json_set(&claude, &["env", k], None)?;
            }
            if tx.changes.is_empty() {
                return Err("Codex 和 Claude Code 里都没有代理设置，无需清除".into());
            }
            "清除 Codex / Claude Code 代理".into()
        }
        "claude_ca" => {
            let path = p.path.as_deref().map(str::trim).unwrap_or("");
            if path.is_empty() || path.contains(['\n', '\r']) || !Path::new(path).is_file() {
                return Err("找不到这个证书文件，请填写 .pem / .crt 文件的完整路径".into());
            }
            tx.json_set(&claude, &["env", "NODE_EXTRA_CA_CERTS"], s(path))?;
            "Claude Code 信任自定义证书".into()
        }
        "codex_sandbox" => {
            tx.toml_set(&codex_toml, &["windows", "sandbox"], Some("unelevated".into()))?;
            "Codex 沙盒改为 unelevated".into()
        }
        "codex_mcp_timeout" => {
            let list = mcp_timeouts(&codex_toml)?;
            if list.is_empty() {
                return Err("Codex 里没有配置 MCP 服务器".into());
            }
            for (name, cur) in list {
                if cur.map_or(true, |n| n < MCP_STARTUP_SEC as f64) {
                    tx.toml_set(&codex_toml, &["mcp_servers", &name, "startup_timeout_sec"], Some(MCP_STARTUP_SEC.into()))?;
                }
            }
            format!("Codex MCP 启动超时设为 {MCP_STARTUP_SEC} 秒")
        }
        "claude_git_bash" => {
            let path = match p.path.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
                Some(x) => PathBuf::from(x),
                None => git_bash().ok_or("没有找到 Git Bash，请先在「安装助手」里安装 Git")?,
            };
            if !path.is_file() {
                return Err(format!("找不到 {}", path.display()));
            }
            tx.json_set(&claude, &["env", "CLAUDE_CODE_GIT_BASH_PATH"], s(&path.to_string_lossy()))?;
            "Claude Code 指定 Git Bash 路径".into()
        }
        "claude_autoupdate_off" => {
            tx.json_set(&claude, &["env", "DISABLE_AUTOUPDATER"], s("1"))?;
            "Claude Code 关闭自动更新".into()
        }
        "claude_stable" => {
            tx.json_set(&claude, &["autoUpdatesChannel"], s("stable"))?;
            "Claude Code 切换到 stable 渠道".into()
        }
        "claude_max_output" => {
            tx.json_set(&claude, &["env", "CLAUDE_CODE_MAX_OUTPUT_TOKENS"], s("32000"))?;
            "Claude Code 输出上限设为 32000".into()
        }
        "claude_mcp_timeout" => {
            tx.json_set(&claude, &["env", "MCP_TIMEOUT"], s("60000"))?;
            "Claude Code MCP 超时设为 60 秒".into()
        }
        "codex_retries" => {
            let doc = parse_toml(read_opt(&codex_toml)?.as_deref(), &codex_toml)?;
            let prov = custom_provider(&doc).ok_or(OPENAI_RETRY_MSG)?;
            for (k, n) in [("stream_max_retries", STREAM_RETRIES), ("request_max_retries", REQUEST_RETRIES)] {
                if retries_of(&doc, &prov, k).map_or(true, |c| c < n) {
                    tx.toml_set(&codex_toml, &["model_providers", &prov, k], Some(n.into()))?;
                }
            }
            format!("Codex 断线后多重试几次（5 → {STREAM_RETRIES}）")
        }
        "codex_chinese" => {
            tx.block_set(&codex_agents_path()?, "zh", Some(ZH_CODEX))?;
            "Codex 改为中文回复".into()
        }
        "claude_chinese" => {
            tx.json_set(&claude, &["language"], s(ZH_CLAUDE))?;
            "Claude Code 改为中文回复".into()
        }
        "codex_ui_zh" => {
            tx.toml_set(&codex_toml, &["desktop", "localeOverride"], Some(CODEX_UI_LOCALE.into()))?;
            "Codex 桌面版界面改为中文".into()
        }
        "claude_ui_zh" => {
            claude_ui_zh(&mut tx)?;
            "Claude 桌面版开启中文（重启后在「设置 → 语言」选择中文）".into()
        }
        "zh_all" => {
            tx.toml_set(&codex_toml, &["desktop", "localeOverride"], Some(CODEX_UI_LOCALE.into()))?;
            tx.block_set(&codex_agents_path()?, "zh", Some(ZH_CODEX))?;
            tx.json_set(&claude, &["language"], s(ZH_CLAUDE))?;
            match claude_ui_zh(&mut tx) {
                Ok(()) => "一键中文（Codex 界面和回复、Claude 桌面版界面、Claude Code 回复）".into(),
                Err(e) => format!("一键中文（Codex 界面和回复、Claude Code 回复；Claude 桌面版跳过：{e}）"),
            }
        }
        _ => return Err("未知的修复项".into()),
    };
    let msg = commit(&id, title, tx)?;
    Ok(if id == "codex_retries" && msg.ends_with("已完成") {
        msg + "。不会增加断线次数，只是断线后多尝试重连再放弃；经常断线请让 Codex 走本机代理"
    } else {
        msg
    })
}

/// PowerShell single-quoted literal.
fn psq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn run_ok(prog: &Path, args: &[&str]) -> R<()> {
    let mut c = Command::new(prog);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = quiet(&mut c).output().map_err(|e| format!("无法运行 {}：{e}", prog.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(format!("{} 执行失败{}", prog.display(), if why.is_empty() { String::new() } else { format!("：{why}") }))
    }
}

/// Reset .codex permissions (fixes "ChatGPT 需要一次性权限 / Windows 安装未完成").
/// The old ACLs are saved with icacls /save first, so undo restores them.
#[cfg(windows)]
fn acl_reset() -> R<String> {
    let dir = codex_dir()?;
    if !dir.is_dir() {
        return Err("没有找到 Codex 配置文件夹 .codex".into());
    }
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() || user.chars().any(|c| "\"<>|:*?\\/".contains(c)) {
        return Err("无法确定当前 Windows 用户名".into());
    }
    let icacls = Path::new("icacls.exe");
    let mut list = load_records()?;
    let mut time = now_ms();
    while list.iter().any(|r| r.time == time) {
        time += 1;
    }
    let id = format!("{time}-codex_acl_reset");
    let rdir = fixes_dir()?.join(&id);
    fs::create_dir_all(&rdir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let d = dir.to_string_lossy().to_string();
    let save = format!(
        "$t={}; @(Get-Item -LiteralPath $t -Force)+@(Get-ChildItem -LiteralPath $t -Recurse -Force) | ForEach-Object {{ [pscustomobject]@{{p=$_.FullName; s=(Get-Acl -LiteralPath $_.FullName).GetSecurityDescriptorSddlForm('Access')}} }} | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath {} -Encoding UTF8",
        psq(&d),
        psq(&rdir.join("acl.json").to_string_lossy())
    );
    let saved = run_ok(Path::new("powershell.exe"), &["-NoProfile", "-NonInteractive", "-Command", &save]).and_then(|_| {
        if rdir.join("acl.json").is_file() { Ok(()) } else { Err("没有生成备份文件".into()) }
    });
    if let Err(e) = saved {
        let _ = fs::remove_dir_all(&rdir);
        return Err(format!("备份原权限失败，没有做任何修改。{e}"));
    }
    let title = "重置 Codex 文件夹权限".to_string();
    list.insert(0, FixRecord { id, fix: "codex_acl_reset".into(), title: title.clone(), time, files: vec![], changes: vec![], acl: Some(d.clone()) });
    save_records(&list)?;
    run_ok(icacls, &[&d, "/reset", "/T", "/C"]).map_err(|e| format!("{e}。原权限已备份，可在修复记录里撤回"))?;
    run_ok(icacls, &[&d, "/grant", &format!("{user}:(OI)(CI)F"), "/C"]).map_err(|e| format!("{e}。原权限已备份，可在修复记录里撤回"))?;
    Ok(format!("{title}：已完成。请完全退出 Codex 后重新打开"))
}

#[cfg(not(windows))]
fn acl_reset() -> R<String> {
    Err("只有 Windows 上需要这个修复".into())
}

/// .codex exists but this user cannot write into it.
fn codex_unwritable() -> bool {
    let Ok(d) = codex_dir() else { return false };
    if !d.is_dir() {
        return false;
    }
    let p = d.join(".xbtry-write-test");
    match fs::write(&p, b"x") {
        Ok(()) => {
            let _ = fs::remove_file(&p);
            false
        }
        Err(_) => true,
    }
}

#[derive(Serialize)]
pub struct FixItem {
    id: String,
    title: String,
    time: u64,
    files: Vec<String>,
}

#[tauri::command]
pub fn list_fixes() -> R<Vec<FixItem>> {
    Ok(load_records()?
        .into_iter()
        .map(|r| FixItem { id: r.id, title: r.title, time: r.time, files: r.files.iter().map(|f| f.path.display().to_string()).collect() })
        .collect())
}

#[tauri::command]
pub fn undo_fix(id: String) -> R<String> {
    let mut list = load_records()?;
    let pos = list.iter().position(|r| r.id == id).ok_or("找不到这条修复记录，可能已经撤回过了")?;
    let rec = list[pos].clone();
    let dir = fixes_dir()?.join(&rec.id);
    // Prepare everything first, write afterwards.
    let mut writes: Vec<(PathBuf, Option<String>)> = vec![];
    let mut partial = false;
    for f in &rec.files {
        let cur = read_opt(&f.path)?;
        if cur.as_deref().map(fingerprint).as_deref() == Some(f.after.as_str()) {
            let before = match &f.before {
                Some(n) => Some(fs::read_to_string(dir.join(n)).map_err(|e| format!("读取备份失败：{e}"))?),
                None => None,
            };
            writes.push((f.path.clone(), before));
        } else {
            partial = true;
            let mut tx = Tx::default();
            for c in rec.changes.iter().rev().filter(|c| c.file == f.path) {
                tx.revert(c)?;
            }
            if let Some((p, _, after)) = tx.files.pop() {
                writes.push((p, Some(after)));
            }
        }
    }
    for (p, text) in writes {
        match text {
            Some(t) => write_atomic(&p, &t)?,
            None if p.exists() => fs::remove_file(&p).map_err(|e| format!("删除 {} 失败：{e}", p.display()))?,
            None => {}
        }
    }
    if rec.acl.is_some() {
        // Put back only the access rules (owner untouched, works without admin rights).
        let bk = psq(&dir.join("acl.json").to_string_lossy());
        let script = format!("$list = Get-Content -LiteralPath {bk} -Raw | ConvertFrom-Json; foreach($e in $list){{ if(Test-Path -LiteralPath $e.p){{ $a=Get-Acl -LiteralPath $e.p; $a.SetSecurityDescriptorSddlForm($e.s,'Access'); Set-Acl -LiteralPath $e.p -AclObject $a }} }}");
        run_ok(Path::new("powershell.exe"), &["-NoProfile", "-NonInteractive", "-Command", &script])?;
    }
    list.remove(pos);
    save_records(&list)?;
    let _ = fs::remove_dir_all(&dir);
    Ok(if partial {
        format!("已撤回「{}」。文件之后又被改过，所以只还原了这次修改的设置项", rec.title)
    } else {
        format!("已撤回「{}」", rec.title)
    })
}
// ---------- checks ----------

fn client_checks(out: &mut Vec<Check>) {
    let tools = [
        ("codex", find_tool("codex").or_else(codex_desktop_bin)),
        ("claude", find_tool("claude")),
        ("git", find_tool("git")),
        ("node", find_tool("node")),
    ];
    let versions: Vec<Option<String>> = std::thread::scope(|s| {
        let hs: Vec<_> = tools.iter().map(|(_, p)| s.spawn(move || p.as_deref().and_then(version_of))).collect();
        hs.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    let g = "客户端";
    for ((name, path), ver) in tools.iter().zip(versions) {
        let found = |label: &str| {
            let p = path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            check(g, label, Level::Ok, format!("{} · {p}", ver.clone().unwrap_or_else(|| "版本未知".into())))
        };
        out.push(match (*name, path.is_some()) {
            ("codex", true) => found("Codex"),
            ("codex", false) if codex_dir().is_ok_and(|d| d.is_dir()) => {
                check(g, "Codex", Level::Info, "有 Codex 配置目录，但没找到 codex 命令（只装了桌面版时正常）")
            }
            ("codex", false) => check(g, "Codex", Level::Warn, "没有检测到 Codex").open("install"),
            ("claude", true) => found("Claude Code"),
            ("claude", false) => check(g, "Claude Code", Level::Warn, "没有检测到 Claude Code 命令行").open("install"),
            ("git", true) => found("Git"),
            ("git", false) if cfg!(windows) => check(g, "Git", Level::Warn, "没有检测到 Git。Windows 上的 Claude Code 需要 Git Bash").open("install"),
            ("git", false) => check(g, "Git", Level::Info, "没有检测到 Git"),
            (_, true) => found("Node.js"),
            _ => check(g, "Node.js", Level::Info, "没有检测到 Node.js。只有 npm 安装方式和部分 MCP 服务器需要它"),
        });
    }
}

fn codex_config_checks(out: &mut Vec<Check>) {
    let g = "配置";
    let Ok(path) = codex_toml_path() else { return };
    if cfg!(windows) && codex_unwritable() {
        out.push(
            check(g, "Codex 文件夹权限", Level::Err, "当前用户没法写入 .codex 文件夹，Codex 会提示「需要一次性权限」或「Windows 安装未完成」")
                .fix(offer("codex_acl_reset", "重置 .codex 权限", Value::Null)),
        );
    }
    let text = match read_opt(&path) {
        Ok(Some(t)) => t,
        Ok(None) => {
            out.push(check(g, "Codex 配置", Level::Info, "还没有 config.toml，可以用「API 导入」生成").open("import"));
            return;
        }
        Err(e) => {
            out.push(check(g, "Codex 配置", Level::Err, e));
            return;
        }
    };
    let doc = match text.trim_start_matches('\u{feff}').parse::<DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            let first = e.to_string().lines().next().unwrap_or("").to_string();
            out.push(check(g, "Codex 配置", Level::Err, format!("config.toml 无法解析，Codex 会启动失败：{first}")));
            return;
        }
    };
    let s = |item: Option<&Item>| item.and_then(Item::as_str).map(str::to_string);
    let provider = s(doc.get("model_provider"));
    let model = s(doc.get("model")).unwrap_or_else(|| "默认".into());
    out.push(match provider.as_deref() {
        Some(PROVIDER_ID) => check(g, "Codex 配置", Level::Ok, format!("已指向 XBTry · 模型 {model}")),
        Some(p) => check(g, "Codex 配置", Level::Info, format!("当前使用 {p}，不是 XBTry")).open("import"),
        None => check(g, "Codex 配置", Level::Info, "当前使用官方登录，没有接入 XBTry").open("import"),
    });
    if let Some(name) = s(doc.get("profile")) {
        let prof = doc.get("profiles").and_then(|p| p.get(&name));
        let over: Vec<&str> = ["model_provider", "model"].into_iter().filter(|k| prof.and_then(|p| p.get(k)).is_some()).collect();
        if !over.is_empty() {
            out.push(check(g, "Codex profile", Level::Warn, format!("默认 profile「{name}」会覆盖 {}，XBTry 的设置可能不生效", over.join("、"))));
        }
    }
    if let Some(p) = custom_provider(&doc) {
        let n = retries_of(&doc, &p, "stream_max_retries").unwrap_or(5);
        out.push(if n < STREAM_RETRIES {
            check(g, "Codex 断线重连", Level::Info, format!("{p} 断线后最多自动重试 {n} 次就放弃，网络一抖任务就可能中断。多重试不会增加断线次数"))
                .fix(offer("codex_retries", "断线时多重试几次（5 → 20）", Value::Null))
        } else {
            check(g, "Codex 断线重连", Level::Ok, format!("断线后最多自动重试 {n} 次"))
        });
    }
    if cfg!(windows) {
        let mode = doc.get("windows").and_then(|w| w.get("sandbox")).and_then(Item::as_str);
        out.push(match mode {
            Some("elevated") => check(g, "Codex 沙盒", Level::Warn, "沙盒模式是 elevated。出现「setup refresh had errors」时改成 unelevated 即可")
                .fix(offer("codex_sandbox", "改为 unelevated", Value::Null)),
            Some(m) => check(g, "Codex 沙盒", Level::Ok, format!("沙盒模式 {m}")),
            None => check(g, "Codex 沙盒", Level::Ok, "沙盒模式为默认"),
        });
    }
    if let Ok(list) = mcp_timeouts(&path) {
        let short: Vec<String> = list.iter().filter(|(_, n)| n.map_or(true, |n| n < MCP_STARTUP_SEC as f64)).map(|(k, _)| k.clone()).collect();
        if !short.is_empty() {
            out.push(
                check(g, "Codex MCP", Level::Info, format!("{} 的启动超时不到 {MCP_STARTUP_SEC} 秒（默认 10 秒），慢的 MCP 会启动失败", short.join("、")))
                    .fix(offer("codex_mcp_timeout", "改为 60 秒", Value::Null)),
            );
        }
    }
}

fn claude_config_checks(out: &mut Vec<Check>) {
    let g = "配置";
    let Ok(path) = claude_settings_path() else { return };
    let settings = match read_json_object(&path) {
        Ok(s) => s,
        Err(e) => {
            out.push(check(g, "Claude Code 配置", Level::Err, e));
            return;
        }
    };
    let env = settings.get("env").cloned().unwrap_or(Value::Null);
    let get = |k: &str| env.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
    let base = get("ANTHROPIC_BASE_URL");
    out.push(match base.as_deref().map(|u| u.trim_end_matches('/')) {
        Some(CLAUDE_BASE) => check(g, "Claude Code 配置", Level::Ok, format!("已指向 XBTry · 模型 {}", get("ANTHROPIC_MODEL").unwrap_or_else(|| "默认".into()))),
        Some(u) => check(g, "Claude Code 配置", Level::Info, format!("当前地址 {u}，不是 XBTry")).open("import"),
        None if !path.is_file() => check(g, "Claude Code 配置", Level::Info, "还没有 settings.json，可以用「API 导入」生成").open("import"),
        None => check(g, "Claude Code 配置", Level::Info, "当前使用官方登录，没有接入 XBTry").open("import"),
    });
    if cfg!(windows) && find_tool("claude").is_some() {
        let set = get("CLAUDE_CODE_GIT_BASH_PATH");
        let found = git_bash();
        let on_path = find_in("git", &path_dirs()).is_some();
        out.push(match (set, found) {
            (Some(p), _) if Path::new(&p).is_file() => check(g, "Git Bash", Level::Ok, p),
            (Some(p), Some(f)) => check(g, "Git Bash", Level::Err, format!("设置的路径 {p} 不存在"))
                .fix(offer("claude_git_bash", "改为检测到的路径", json!({ "path": f.display().to_string() }))),
            (Some(p), None) => check(g, "Git Bash", Level::Err, format!("设置的路径 {p} 不存在，也没找到 Git")).open("install"),
            (None, Some(f)) if on_path => check(g, "Git Bash", Level::Ok, f.display().to_string()),
            (None, Some(f)) => check(g, "Git Bash", Level::Warn, format!("Git 不在 PATH 上，Claude Code 可能找不到 {}", f.display()))
                .fix(offer("claude_git_bash", "写入路径", json!({ "path": f.display().to_string() }))),
            (None, None) => check(g, "Git Bash", Level::Err, "没找到 Git Bash，Claude Code 在 Windows 上无法运行命令").open("install"),
        });
    }
}

fn env_names() -> Vec<(&'static str, String)> {
    #[cfg(windows)]
    {
        use winreg::{enums::*, RegKey};
        let mut out = vec![];
        for (scope, root, sub) in [
            ("用户级", HKEY_CURRENT_USER, "Environment"),
            ("系统级", HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        ] {
            if let Ok(k) = RegKey::predef(root).open_subkey(sub) {
                out.extend(k.enum_values().flatten().map(|(n, _)| (scope, n)));
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).map(|k| ("当前环境", k)).collect()
    }
}

fn conflicting(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    ["ANTHROPIC_", "OPENAI_", "CLAUDE_CODE_USE_"].iter().any(|p| u.starts_with(p))
        || matches!(u.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "CODEX_HOME" | "CLAUDE_CONFIG_DIR")
}

fn env_checks(out: &mut Vec<Check>) {
    let hits: Vec<(&str, String)> = env_names().into_iter().filter(|(_, n)| conflicting(n)).collect();
    if hits.is_empty() {
        out.push(check("环境变量", "环境变量", Level::Ok, "没有会干扰配置的 ANTHROPIC_*、OPENAI_* 或代理变量"));
        return;
    }
    let mut parts: Vec<String> = vec![];
    for scope in ["用户级", "系统级", "当前环境"] {
        let names: Vec<&str> = hits.iter().filter(|(s, _)| *s == scope).map(|(_, n)| n.as_str()).collect();
        if !names.is_empty() {
            parts.push(format!("{scope}：{}", names.join("、")));
        }
    }
    out.push(check(
        "环境变量",
        "环境变量",
        Level::Warn,
        format!("{}。这些变量可能盖过配置文件，出问题时到「系统属性 → 环境变量」里删除后重启客户端", parts.join("；")),
    ));
}

fn network_checks(out: &mut Vec<Check>) {
    let g = "网络";
    let info = proxy_info();
    let sys = match (&info.system, info.system_on) {
        (Some(s), true) => format!("系统代理已开启：{s}"),
        (Some(s), false) => format!("系统代理未开启（记录的地址 {s}）"),
        (None, _) => "没有系统代理".into(),
    };
    let detail = if info.listening.is_empty() {
        format!("{sys}；本机没有发现正在运行的代理端口")
    } else {
        format!("{sys}；正在监听：{}", info.listening.join("、"))
    };
    out.push(check(g, "本机代理", Level::Ok, detail));
    let dead = |u: &str| local_port(u).is_some_and(|p| !info.listening.iter().any(|l| local_port(l) == Some(p)));
    for (name, cur, fix_id) in [("Codex 代理", &info.codex, "codex_proxy"), ("Claude Code 代理", &info.claude, "claude_proxy")] {
        match cur {
            Some(u) if dead(u) => {
                out.push(check(g, name, Level::Err, format!("设置了 {u}，但这个端口没有在监听，客户端会连不上")).fix(offer("proxy_clear", "清除代理", Value::Null)).open("proxy"));
            }
            Some(u) => out.push(check(g, name, Level::Ok, format!("走代理 {u}"))),
            None => {
                let mut c = check(g, name, Level::Ok, "直连，没有设置代理");
                if let Some(s) = &info.suggested {
                    c = check(g, name, Level::Info, format!("直连。经常断线或 stream disconnected 时，可以让它走本机代理 {s}"))
                        .fix(offer(fix_id, "使用这个代理", json!({ "url": s })));
                }
                out.push(c.open("proxy"));
            }
        }
    }
}

fn language_checks(out: &mut Vec<Check>) {
    let g = "语言";
    if let Ok(p) = codex_toml_path() {
        let doc = read_opt(&p).ok().flatten().unwrap_or_default().trim_start_matches('\u{feff}').parse::<DocumentMut>().ok();
        if let Some(doc) = doc {
            let cur = doc.get("desktop").and_then(|d| d.get("localeOverride")).and_then(Item::as_str).map(str::to_string);
            out.push(match cur.as_deref() {
                Some(l) if l.to_ascii_lowercase().starts_with("zh") => check(g, "Codex 界面语言", Level::Ok, format!("桌面版界面为中文（{l}）")),
                Some(l) => check(g, "Codex 界面语言", Level::Info, format!("桌面版界面语言 {l}")).fix(offer("codex_ui_zh", "改为中文", Value::Null)),
                None => check(g, "Codex 界面语言", Level::Info, "跟随系统语言，系统是英文时界面就是英文").fix(offer("codex_ui_zh", "改为中文", Value::Null)),
            });
        }
    }
    let cfgs = claude_desktop_configs();
    if !cfgs.is_empty() {
        let locs: Vec<String> = cfgs
            .iter()
            .filter_map(|p| read_json_object(p).ok()?.get("locale")?.as_str().map(str::to_string))
            .collect();
        // Every folder Claude may read must be Chinese, otherwise gateway mode stays English.
        let all_zh = cfgs.iter().all(|p| {
            read_json_object(p).ok().and_then(|m| m.get("locale").and_then(Value::as_str).map(|l| l.to_ascii_lowercase().starts_with("zh"))).unwrap_or(false)
        });
        out.push(if claude_gateway_mode() {
            check(g, "Claude 桌面版界面语言", Level::Info, CLAUDE_GATEWAY_ZH)
        } else if all_zh {
            check(g, "Claude 桌面版界面语言", Level::Ok, format!("已开启中文（{}）。界面还是英文时，完全退出 Claude 再打开，到「设置 → 语言」选择中文", locs.join("、")))
        } else {
            let cur = locs.first().cloned().unwrap_or_else(|| "en-US".into());
            check(g, "Claude 桌面版界面语言", Level::Info, format!("当前 {cur}，改中文前要先完全退出 Claude 桌面版"))
                .fix(offer("claude_ui_zh", "改为中文", Value::Null))
        });
    }
    if let Ok(p) = codex_agents_path() {
        let text = read_opt(&p).ok().flatten().unwrap_or_default();
        out.push(if block_get(&text, "zh").is_some() || text.contains("中文") {
            check(g, "Codex 回复语言", Level::Ok, format!("{} 里已要求用中文回复", p.display()))
        } else {
            check(g, "Codex 回复语言", Level::Info, "没有设置，Codex 经常用英文回复").fix(offer("codex_chinese", "改为中文回复", Value::Null))
        });
    }
    if let Ok(p) = claude_settings_path() {
        if let Ok(s) = read_json_object(&p) {
            out.push(match s.get("language").and_then(Value::as_str).filter(|v| !v.trim().is_empty()) {
                Some(v) => check(g, "Claude Code 回复语言", Level::Ok, format!("language = {v}")),
                None => check(g, "Claude Code 回复语言", Level::Info, "没有设置，回复语言跟随提问")
                    .fix(offer("claude_chinese", "改为中文回复", Value::Null)),
            });
        }
    }
}

/// Only the language items, for 一键中文.
pub(crate) fn language_check_list() -> Vec<Check> {
    let mut out = vec![];
    language_checks(&mut out);
    out
}

fn local_checks() -> Vec<Check> {
    let mut out = vec![];
    client_checks(&mut out);
    codex_config_checks(&mut out);
    claude_config_checks(&mut out);
    language_checks(&mut out);
    env_checks(&mut out);
    network_checks(&mut out);
    out
}

/// The key already imported, so a check can run without typing it again.
fn saved_key() -> Option<String> {
    let dir = codex_dir().ok()?;
    let doc = read_opt(&dir.join("config.toml")).ok().flatten().unwrap_or_default().parse::<DocumentMut>().unwrap_or_default();
    if doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_ID) {
        if let Some(k) = read_json_object(&dir.join("auth.json")).ok()?.get("OPENAI_API_KEY").and_then(Value::as_str) {
            return Some(k.to_string());
        }
    }
    (claude_env("ANTHROPIC_BASE_URL")?.trim_end_matches('/') == CLAUDE_BASE).then(|| claude_env("ANTHROPIC_AUTH_TOKEN")).flatten()
}

async fn relay_check(client: reqwest::Client, key: Option<String>) -> Check {
    let g = "服务";
    let t = "XBTry 中转站";
    let mut req = client.get(format!("{CODEX_BASE}/models"));
    if let Some(k) = &key {
        req = req.bearer_auth(k);
    }
    let res = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let why = if e.is_timeout() { "连接超时" } else { "无法连接" };
            return check(g, t, Level::Err, format!("{why} api.xbtry.com。检查网络，或在「代理设置」里配置代理")).open("proxy");
        }
    };
    let code = res.status().as_u16();
    match (&key, code) {
        (None, _) => check(g, t, Level::Ok, format!("api.xbtry.com 可以连通（HTTP {code}）。还没接入，没有测试密钥")),
        (Some(_), 401 | 403) => check(g, t, Level::Err, "API Key 无效或已过期，请到「API 导入」重新填写").open("import"),
        (Some(_), 200..=299) => {
            let body: Value = res.json().await.unwrap_or(Value::Null);
            let n = body.get("data").and_then(Value::as_array).map_or(0, Vec::len);
            check(g, t, Level::Ok, format!("密钥有效，可用模型 {n} 个"))
        }
        (Some(_), c) => check(g, t, Level::Warn, format!("中转站返回 HTTP {c}，稍后再试")),
    }
}

async fn status_page(client: reqwest::Client, title: &'static str, url: &'static str) -> Check {
    let g = "服务";
    let body: Option<Value> = match client.get(url).send().await {
        Ok(r) if r.status().is_success() => r.json().await.ok(),
        _ => None,
    };
    let Some(body) = body else {
        return check(g, title, Level::Info, "状态页打不开，跳过");
    };
    let desc = body["status"]["description"].as_str().unwrap_or("").to_string();
    let level = match body["status"]["indicator"].as_str().unwrap_or("") {
        "none" => Level::Ok,
        "minor" | "maintenance" => Level::Warn,
        _ => Level::Err,
    };
    let desc = if desc == "All Systems Operational" { "官方服务正常".to_string() } else { format!("官方状态：{desc}") };
    check(g, title, level, desc)
}

#[tauri::command]
pub async fn run_checks(key: Option<String>) -> R<Vec<Check>> {
    let key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).or_else(saved_key);
    let local = tauri::async_runtime::spawn_blocking(local_checks);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(8)).build().map_err(|e| e.to_string())?;
    let hs = [
        tauri::async_runtime::spawn(relay_check(client.clone(), key)),
        tauri::async_runtime::spawn(status_page(client.clone(), "OpenAI 状态", "https://status.openai.com/api/v2/status.json")),
        tauri::async_runtime::spawn(status_page(client, "Claude 状态", "https://status.claude.com/api/v2/status.json")),
        tauri::async_runtime::spawn(ip_as_check()),
    ];
    let mut out = local.await.map_err(|e| e.to_string())?;
    for h in hs {
        if let Ok(c) = h.await {
            out.push(c);
        }
    }
    Ok(out)
}
// ---------- error diagnosis ----------

#[derive(Serialize, Debug)]
pub struct Diagnosis {
    title: &'static str,
    cause: String,
    fix: Option<FixOffer>,
    open: Option<&'static str>,
}

fn any(text: &str, pats: &[&str]) -> bool {
    pats.iter().any(|p| text.contains(p))
}

#[tauri::command]
pub fn diagnose(text: String) -> Vec<Diagnosis> {
    let t = text.to_lowercase();
    let mut out = vec![];
    if t.trim().is_empty() {
        return out;
    }
    let mut add = |title, cause: &str, fix: Option<FixOffer>, open| out.push(Diagnosis { title, cause: cause.into(), fix, open });
    if any(&t, &["setup refresh had errors", "windows sandbox", "sandbox setup", "codex-windows-sandbox"]) {
        add("Codex 沙盒初始化失败", "Windows 上 elevated 沙盒初始化出错。改成 unelevated 后重启 Codex。", Some(offer("codex_sandbox", "改为 unelevated", Value::Null)), None);
    }
    if t.contains("mcp") && any(&t, &["timed out", "timeout", "failed to start", "startup", "超时"]) {
        if any(&t, &["mcp client for", "startup_timeout", "codex"]) {
            add("Codex MCP 启动超时", "MCP 服务器默认只有 10 秒启动时间，首次下载依赖时常常不够。", Some(offer("codex_mcp_timeout", "改为 60 秒", Value::Null)), None);
        } else {
            add("Claude Code MCP 超时", "MCP 服务器启动或响应太慢。把 MCP_TIMEOUT 调到 60 秒。", Some(offer("claude_mcp_timeout", "改为 60 秒", Value::Null)), None);
        }
    }
    if any(&t, &["git-bash", "git bash", "requires git", "bash.exe"]) {
        let fix = git_bash().map(|p| offer("claude_git_bash", "写入 Git Bash 路径", json!({ "path": p.display().to_string() })));
        let cause = if fix.is_some() { "Claude Code 找不到 Git Bash，但本机装了 Git，写入路径即可。" } else { "Claude Code 在 Windows 上需要 Git Bash，本机没有找到 Git。" };
        let open = if fix.is_some() { None } else { Some("install") };
        add("找不到 Git Bash", cause, fix, open);
    }
    if any(&t, &["self signed certificate", "self-signed certificate", "unable_to_get_issuer_cert", "unable to get local issuer", "certificate verify failed", "unable_to_verify_leaf_signature", "cert_has_expired"]) {
        add("证书校验失败", "公司网络或代理软件替换了 HTTPS 证书。在「代理设置」里填写根证书文件，让 Claude Code 信任它。", None, Some("proxy"));
    }
    if any(&t, &["output token maximum", "max_output_tokens", "exceeded the 32000", "response exceeded"]) {
        add("输出超过上限", "单次回复超过了默认输出上限。把 CLAUDE_CODE_MAX_OUTPUT_TOKENS 设为 32000。", Some(offer("claude_max_output", "设为 32000", Value::Null)), None);
    }
    if any(&t, &["econnrefused", "connection refused", "proxyconnect", "拒绝连接"]) {
        add("代理连不上", "客户端设置了代理，但代理软件没开或端口不对。打开代理软件，或清除客户端里的代理设置。", Some(offer("proxy_clear", "清除代理", Value::Null)), Some("proxy"));
    }
    let reconnect = any(&t, &["reconnecting", "stream disconnected", "retry limit", "exceeded retry"]);
    if reconnect || any(&t, &["error sending request", "connection reset", "econnreset", "socket hang up", "network error", "fetch failed"]) {
        // Root cause first: the long-lived stream bypassing the local proxy.
        let fix = proxy_info().suggested.map(|u| offer("codex_proxy", "让 Codex 走本机代理", json!({ "url": u })));
        let cause = if fix.is_some() {
            "到服务器的长连接被中断。最常见的原因是开了代理软件，但 Codex 的长连接没走代理。让 Codex 走本机代理能从根上减少断线（只改 Codex，不动系统代理和浏览器）。"
        } else {
            "到服务器的长连接被中断。本机没发现正在运行的代理端口；如果人在海外也一样断线，请运行一键检测查看中转站连通性和出口 IP。"
        };
        add("连接中断", cause, fix, Some("proxy"));
    }
    if reconnect {
        let doc = codex_toml_path().ok().and_then(|p| read_opt(&p).ok().flatten()).and_then(|s| s.parse::<DocumentMut>().ok());
        match doc.as_ref().and_then(|d| custom_provider(d)) {
            Some(_) => add(
                "断线后多重试几次",
                "这一项不会减少断线，只是断线后多尝试重连（5 → 20 次）再放弃，网络短暂抖动时任务不会中断。建议先做上面的「走本机代理」。",
                Some(offer("codex_retries", "断线时多重试几次（5 → 20）", Value::Null)),
                None,
            ),
            None => add("Codex 重连次数用完", OPENAI_RETRY_MSG, None, Some("proxy")),
        }
    }
    if any(&t, &["at capacity", "model is at capacity", "模型已满", "满载"]) {
        add("模型满载", "上游这个模型暂时满载（Selected model is at capacity）。稍等一两分钟重试，或在「API 导入」里换一个模型。不建议安装来路不明的本地重试中转，它能看到你所有的请求和登录凭证。", None, Some("import"));
    }
    if any(&t, &["一次性权限", "安装未完成", "installation incomplete", "access is denied", "拒绝访问", "os error 5"]) {
        add("Codex 文件夹权限异常", "Codex 读写 .codex 文件夹被拒绝（常见提示「ChatGPT 需要一次性权限」「Windows 安装未完成」）。重置 .codex 的权限并授予当前用户完全控制即可。修复前会备份原权限，可以撤回。", Some(offer("codex_acl_reset", "重置 .codex 权限", Value::Null)), None);
    }
    if any(&t, &["session", "accesstoken", "access_token"]) {
        add("当心盗号", "XBTry 不会索要你的密码或 session。任何要求你复制 ChatGPT / Claude 的 session、token 粘贴到第三方网站的做法，都等于把账号交给别人。", None, None);
    }
    if any(&t, &["enotfound", "getaddrinfo", "could not resolve", "dns error", "name or service not known"]) {
        add("域名解析失败", "解析不到 api.xbtry.com。检查网络或 DNS，或在「代理设置」里配置代理。", None, Some("proxy"));
    }
    if any(&t, &["401", "unauthorized", "invalid api key", "invalid_api_key", "incorrect api key", "authentication_error", "invalid x-api-key"]) {
        add("密钥无效", "API Key 填错、过期或被禁用，也可能是系统环境变量里的旧密钥盖过了配置。到「API 导入」重新导入，并运行一键检测查看环境变量。", None, Some("import"));
    }
    if any(&t, &["model_not_found", "model not found", "unknown model", "no available channel", "invalid model"]) {
        add("模型不可用", "当前模型名在中转站上不存在或没有权限。到「API 导入」重新获取模型列表并选择。", None, Some("import"));
    }
    if t.contains("/v1/responses") && t.contains("claude") {
        add("Claude 模型走错接口", "Claude 模型要走 /v1/messages。用「API 导入」配置 Codex，XBTry 会把模型写进 Codex 模型目录。", None, Some("import"));
    }
    if any(&t, &["429", "rate limit", "rate_limit", "insufficient_quota", "quota exceeded", "余额不足", "额度不足"]) {
        add("额度不足或被限流", "请求太频繁或账户余额不足。到 XBTry 后台查看余额，稍等后重试。", None, None);
    }
    if any(&t, &["502", "503", "504", "529", "bad gateway", "overloaded", "service unavailable"]) {
        add("服务端繁忙", "上游服务暂时不可用，一般几分钟后恢复。可以运行一键检测查看官方状态。", None, None);
    }
    if any(&t, &["auto-update", "autoupdate", "update failed", "更新后"]) {
        add("更新导致的问题", "关闭 Claude Code 自动更新，必要时在「版本管理」里回退到 stable 版本。", Some(offer("claude_autoupdate_off", "关闭自动更新", Value::Null)), Some("version"));
    }
    out
}

// ---------- install / version helpers ----------

#[derive(Serialize)]
pub struct ClaudeVersion {
    installed: Option<String>,
    path: Option<String>,
    stable: Option<String>,
    latest: Option<String>,
    autoupdate_off: bool,
    channel: Option<String>,
}

#[tauri::command]
pub async fn claude_version_info() -> R<ClaudeVersion> {
    let local = tauri::async_runtime::spawn_blocking(|| {
        let p = find_tool("claude");
        (p.as_deref().and_then(version_of), p.map(|p| p.display().to_string()))
    });
    let tags: Value = match reqwest::Client::builder().timeout(Duration::from_secs(8)).build() {
        Ok(c) => match c.get("https://registry.npmjs.org/-/package/@anthropic-ai/claude-code/dist-tags").send().await {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or(Value::Null),
            _ => Value::Null,
        },
        Err(_) => Value::Null,
    };
    let (installed, path) = local.await.map_err(|e| e.to_string())?;
    let settings = claude_settings_path().and_then(|p| read_json_object(&p)).unwrap_or_default();
    Ok(ClaudeVersion {
        installed,
        path,
        stable: tags["stable"].as_str().map(str::to_string),
        latest: tags["latest"].as_str().map(str::to_string),
        autoupdate_off: claude_env("DISABLE_AUTOUPDATER").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
        channel: settings.get("autoUpdatesChannel").and_then(Value::as_str).map(str::to_string),
    })
}

fn valid_version(v: &str) -> bool {
    v == "stable" || v == "latest" || {
        let parts: Vec<&str> = v.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 6 && p.chars().all(|c| c.is_ascii_digit()))
    }
}

/// Script for a fixed action. Only whitelisted commands; the version is validated.
fn terminal_script(action: &str, version: Option<&str>) -> R<String> {
    let ver = || -> R<String> {
        let v = version.map(str::trim).unwrap_or("");
        if valid_version(v) { Ok(v.to_string()) } else { Err("版本号格式应为 2.1.311 这样的三段数字".into()) }
    };
    if cfg!(windows) {
        let w = "--accept-source-agreements --accept-package-agreements";
        Ok(match action {
            "install_claude" => format!("winget install --id Anthropic.ClaudeCode -e {w}"),
            "install_git" => format!("winget install --id Git.Git -e {w}"),
            "install_node" => format!("winget install --id OpenJS.NodeJS.LTS -e {w}"),
            "claude_install_version" => format!("& ([scriptblock]::Create((irm https://claude.ai/install.ps1))) {}", ver()?),
            _ => return Err("未知的操作".into()),
        })
    } else {
        Ok(match action {
            "install_claude" => "curl -fsSL https://claude.ai/install.sh | bash".into(),
            "install_git" => "xcode-select --install".into(),
            "install_node" => "open https://nodejs.org/".into(),
            "claude_install_version" => format!("curl -fsSL https://claude.ai/install.sh | bash -s {}", ver()?),
            _ => return Err("未知的操作".into()),
        })
    }
}

/// Opens a visible terminal so the user can watch the installer (and answer UAC).
#[tauri::command]
pub fn run_terminal(action: String, version: Option<String>) -> R<String> {
    let script = terminal_script(&action, version.as_deref())?;
    #[cfg(windows)]
    {
        Command::new("powershell.exe")
            .args(["-NoExit", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map_err(|e| format!("无法打开 PowerShell：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let f = std::env::temp_dir().join(format!("xbtry-{action}.command"));
        fs::write(&f, format!("#!/bin/bash\n{script}\necho\necho '完成后可以关闭这个窗口，回到 XBTry 重新检测。'\n")).map_err(|e| e.to_string())?;
        let _ = fs::set_permissions(&f, fs::Permissions::from_mode(0o755));
        Command::new("open").arg(&f).spawn().map_err(|e| format!("无法打开终端：{e}"))?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        return Err(format!("请在终端里运行：{script}"));
    }
    #[allow(unreachable_code)]
    Ok(format!("已在新窗口运行：{script}。完成后回到这里重新检测"))
}
// ---------- exit IP / node check (ping0.cc) ----------

/// Regions where Claude / ChatGPT are not offered; using them there risks account bans.
const BAD_REGIONS: [&str; 9] = ["CN", "HK", "MO", "RU", "BY", "IR", "KP", "SY", "CU"];

#[derive(Serialize, Debug, Clone)]
pub struct IpRoute {
    route: String,
    ip: Option<String>,
    location: Option<String>,
    asn: Option<String>,
    org: Option<String>,
    country: Option<String>,
    level: Level,
    note: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct IpReport {
    routes: Vec<IpRoute>,
    level: Level,
}

/// ping0.cc/geo answers "ip\nlocation\nASN\nISP".
fn parse_ping0(text: &str) -> Option<(String, Option<String>, Option<String>, Option<String>)> {
    let l: Vec<&str> = text.lines().map(str::trim).filter(|x| !x.is_empty()).collect();
    let ip = l.first()?;
    if ip.len() > 45 || !ip.chars().all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':') {
        return None;
    }
    let g = |i: usize| l.get(i).map(|s| s.to_string());
    Some((ip.to_string(), g(1), g(2), g(3)))
}

fn region_bad(country: Option<&str>, location: &str) -> bool {
    let cn = ["中国", "香港", "澳门"].iter().any(|k| location.contains(k)) && !location.contains("台湾");
    country.is_some_and(|c| BAD_REGIONS.contains(&c.to_ascii_uppercase().as_str())) || cn
}

async fn ip_via(route: String, proxy: Option<String>) -> IpRoute {
    let mut r = IpRoute { route, ip: None, location: None, asn: None, org: None, country: None, level: Level::Err, note: String::new() };
    let mut b = reqwest::Client::builder().timeout(Duration::from_secs(8)).no_proxy().user_agent("curl/8.0");
    if let Some(p) = &proxy {
        match reqwest::Proxy::all(p) {
            Ok(px) => b = b.proxy(px),
            Err(_) => {
                r.note = format!("代理地址 {p} 无效");
                return r;
            }
        }
    }
    let Ok(c) = b.build() else {
        r.note = "无法创建网络连接".into();
        return r;
    };
    let (c1, c2) = (c.clone(), c);
    let geo = tauri::async_runtime::spawn(async move { c1.get("https://ping0.cc/geo").send().await.ok()?.text().await.ok() });
    let info = tauri::async_runtime::spawn(async move { c2.get("https://ipinfo.io/json").send().await.ok()?.json::<Value>().await.ok() });
    if let Some((ip, loc, asn, org)) = geo.await.ok().flatten().as_deref().and_then(parse_ping0) {
        (r.ip, r.location, r.asn, r.org) = (Some(ip), loc, asn, org);
    }
    if let Some(v) = info.await.ok().flatten() {
        r.country = v["country"].as_str().map(str::to_string);
        if r.ip.is_none() {
            r.ip = v["ip"].as_str().map(str::to_string);
            let loc: Vec<&str> = [v["city"].as_str(), v["region"].as_str(), v["country"].as_str()].into_iter().flatten().collect();
            r.location = (!loc.is_empty()).then(|| loc.join(" "));
            r.org = v["org"].as_str().map(str::to_string);
        }
    }
    let bad = region_bad(r.country.as_deref(), r.location.as_deref().unwrap_or(""));
    (r.level, r.note) = match (&r.ip, bad) {
        (None, _) if proxy.is_some() => (Level::Err, "通过这个代理连不上，代理软件没开或节点失效".into()),
        (None, _) => (Level::Err, "连不上外网检测站".into()),
        (Some(_), true) => (Level::Err, "出口在 Claude / ChatGPT 不支持的地区，直接使用容易被封号，请切换节点".into()),
        (Some(_), false) => (Level::Ok, "出口地区可以正常使用".into()),
    };
    r
}

/// Every way traffic can leave this machine: direct, the system proxy
/// (Claude / Codex desktop use it), and the proxies set inside the clients.
async fn ip_report() -> IpReport {
    let info = tauri::async_runtime::spawn_blocking(proxy_info).await.ok();
    let mut routes: Vec<(String, Option<String>)> = vec![("直连".into(), None)];
    if let Some(i) = &info {
        let mut add = |name: String, url: &Option<String>| {
            if let Some(u) = url {
                if !routes.iter().any(|(_, p)| p.as_deref() == Some(u.as_str())) {
                    routes.push((name, Some(u.clone())));
                }
            }
        };
        if i.system_on {
            add(format!("系统代理 {}", i.system.clone().unwrap_or_default()), &i.system);
        }
        add(format!("Codex 代理 {}", i.codex.clone().unwrap_or_default()), &i.codex);
        add(format!("Claude Code 代理 {}", i.claude.clone().unwrap_or_default()), &i.claude);
        if !i.system_on {
            if let Some(s) = &i.suggested {
                add(format!("本机代理 {s}（未开系统代理）"), &Some(s.clone()));
            }
        }
    }
    let hs: Vec<_> = routes.into_iter().map(|(n, p)| tauri::async_runtime::spawn(ip_via(n, p))).collect();
    let mut out = vec![];
    for h in hs {
        if let Ok(r) = h.await {
            out.push(r);
        }
    }
    // What the desktop apps actually use: the system proxy when it is on, otherwise direct.
    let main = out.iter().find(|r| r.route.starts_with("系统代理")).or_else(|| out.first());
    let level = main.map_or(Level::Err, |r| r.level);
    IpReport { routes: out, level }
}

#[tauri::command]
pub async fn check_ip() -> R<IpReport> {
    Ok(ip_report().await)
}

async fn ip_as_check() -> Check {
    let rep = ip_report().await;
    let main = rep.routes.iter().find(|r| r.route.starts_with("系统代理")).or_else(|| rep.routes.first());
    let Some(m) = main else {
        return check("网络", "出口 IP", Level::Info, "没有完成检测");
    };
    let desc = format!(
        "{} · {} · {}{} · {}",
        m.route,
        m.ip.clone().unwrap_or_else(|| "—".into()),
        m.location.clone().unwrap_or_default(),
        m.org.as_ref().map(|o| format!(" {o}")).unwrap_or_default(),
        m.note
    );
    check("网络", "出口 IP（ping0.cc）", m.level, desc).open("launch")
}

// ---------- Claude launcher ----------

#[derive(Serialize, Debug, Clone)]
pub struct ClaudeApp {
    kind: &'static str,
    label: String,
    path: String,
    /// Store package family, launched through shell:AppsFolder.
    family: Option<String>,
}

fn find_claude_apps() -> Vec<ClaudeApp> {
    let mut out = vec![];
    #[cfg(windows)]
    {
        if let Some(local) = env_dir("LOCALAPPDATA") {
            if let Ok(rd) = fs::read_dir(local.join("Packages")) {
                for e in rd.flatten() {
                    let fam = e.file_name().to_string_lossy().to_string();
                    if fam.starts_with("Claude_") || fam.starts_with("AnthropicPBC.Claude_") {
                        out.push(ClaudeApp { kind: "desktop", label: "Claude 桌面版（应用商店版）".into(), path: format!("shell:AppsFolder\\{fam}!Claude"), family: Some(fam) });
                    }
                }
            }
            for p in [local.join("AnthropicClaude").join("claude.exe"), local.join("Programs").join("Claude").join("Claude.exe")] {
                if p.is_file() {
                    out.push(ClaudeApp { kind: "desktop", label: "Claude 桌面版".into(), path: p.display().to_string(), family: None });
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let p = PathBuf::from("/Applications/Claude.app");
        if p.is_dir() {
            out.push(ClaudeApp { kind: "desktop", label: "Claude 桌面版".into(), path: p.display().to_string(), family: None });
        }
    }
    if let Some(p) = find_tool("claude") {
        out.push(ClaudeApp { kind: "cli", label: "Claude Code 命令行".into(), path: p.display().to_string(), family: None });
    }
    out
}

#[tauri::command]
pub async fn find_claude() -> R<Vec<ClaudeApp>> {
    tauri::async_runtime::spawn_blocking(find_claude_apps).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn launch_claude(path: String) -> R<String> {
    // Only launch something find_claude reported, never an arbitrary path.
    let app = find_claude_apps().into_iter().find(|a| a.path == path).ok_or("这个启动项已经不存在，请重新查找")?;
    #[cfg(windows)]
    {
        let r = match (app.kind, &app.family) {
            (_, Some(_)) => Command::new("explorer.exe").arg(&app.path).spawn(),
            ("cli", _) => Command::new("powershell.exe")
                .args(["-NoExit", "-NoProfile", "-Command", &format!("& '{}'", app.path.replace('\'', "''"))])
                .creation_flags(CREATE_NEW_CONSOLE)
                .spawn(),
            _ => Command::new(&app.path).spawn(),
        };
        r.map_err(|e| format!("启动失败：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        let r = if app.kind == "cli" {
            let f = std::env::temp_dir().join("xbtry-claude.command");
            fs::write(&f, format!("#!/bin/bash\n'{}'\n", app.path.replace('\'', "'\\''"))).map_err(|e| e.to_string())?;
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&f, fs::Permissions::from_mode(0o755));
            Command::new("open").arg(&f).spawn()
        } else {
            Command::new("open").arg(&app.path).spawn()
        };
        r.map_err(|e| format!("启动失败：{e}"))?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Command::new(&app.path).spawn().map_err(|e| format!("启动失败：{e}"))?;
    }
    Ok(format!("已启动 {}", app.label))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HOME_LOCK;

    fn sandbox(tag: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!("xbtry-doctor-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join(".codex")).unwrap();
        fs::create_dir_all(tmp.join(".claude")).unwrap();
        std::env::set_var("XBTRY_HOME", &tmp);
        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        tmp
    }

    const TOML: &str = "# my config\nmodel = \"gpt-6.1-sol\"\n\n[mcp_servers.fs]\ncommand = \"npx\"\n\n[mcp_servers.slow]\ncommand = \"uvx\"\nstartup_timeout_sec = 120\n";
    const SETTINGS: &str = "{\n  \"theme\": \"dark\",\n  \"env\": {\n    \"FOO\": \"1\"\n  }\n}\n";
    const ENV: &str = "# keep me\nFOO=bar\n";

    fn params(url: Option<&str>, path: Option<String>) -> Option<FixParams> {
        Some(FixParams { url: url.map(str::to_string), path })
    }

    #[test]
    fn every_fix_applies_and_undoes_exactly() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("all");
        let (toml_p, set_p, env_p) = (tmp.join(".codex/config.toml"), tmp.join(".claude/settings.json"), tmp.join(".codex/.env"));
        fs::write(&toml_p, TOML).unwrap();
        fs::write(&set_p, SETTINGS).unwrap();
        fs::write(&env_p, ENV).unwrap();
        let ca = tmp.join("ca.pem");
        fs::write(&ca, "x").unwrap();
        let bash = tmp.join("bash.exe");
        fs::write(&bash, "").unwrap();
        let fixes: Vec<(&str, Option<FixParams>)> = vec![
            ("codex_proxy", params(Some("http://127.0.0.1:7897/"), None)),
            ("claude_proxy", params(Some("http://127.0.0.1:7897"), None)),
            ("claude_ca", params(None, Some(ca.display().to_string()))),
            ("codex_sandbox", None),
            ("codex_mcp_timeout", None),
            ("claude_git_bash", params(None, Some(bash.display().to_string()))),
            ("claude_autoupdate_off", None),
            ("claude_stable", None),
            ("claude_max_output", None),
            ("claude_mcp_timeout", None),
            ("codex_chinese", None),
            ("claude_chinese", None),
        ];
        for (id, p) in fixes {
            apply_fix(id.into(), p).unwrap_or_else(|e| panic!("{id}: {e}"));
            let recs = list_fixes().unwrap();
            assert_eq!(recs.len(), 1, "{id}");
            undo_fix(recs[0].id.clone()).unwrap();
            assert_eq!(fs::read_to_string(&toml_p).unwrap(), TOML, "{id}");
            assert_eq!(fs::read_to_string(&set_p).unwrap(), SETTINGS, "{id}");
            assert_eq!(fs::read_to_string(&env_p).unwrap(), ENV, "{id}");
            assert!(!tmp.join(".codex/AGENTS.md").exists(), "{id}");
            assert!(list_fixes().unwrap().is_empty());
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn fixes_write_expected_values_and_survive_later_edits() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("edit");
        let toml_p = tmp.join(".codex/config.toml");
        fs::write(&toml_p, TOML).unwrap();
        // Files that did not exist are removed again on undo.
        apply_fix("codex_proxy".into(), params(Some("http://127.0.0.1:7890"), None)).unwrap();
        assert_eq!(fs::read_to_string(tmp.join(".codex/.env")).unwrap(), "HTTP_PROXY=http://127.0.0.1:7890\nHTTPS_PROXY=http://127.0.0.1:7890\n");
        apply_fix("codex_sandbox".into(), None).unwrap();
        apply_fix("codex_mcp_timeout".into(), None).unwrap();
        let doc: DocumentMut = fs::read_to_string(&toml_p).unwrap().parse().unwrap();
        assert_eq!(doc["windows"]["sandbox"].as_str(), Some("unelevated"));
        assert!(doc["windows"].is_table(), "explicit [windows] table");
        assert_eq!(doc["mcp_servers"]["fs"]["startup_timeout_sec"].as_integer(), Some(60));
        assert_eq!(doc["mcp_servers"]["slow"]["startup_timeout_sec"].as_integer(), Some(120), "longer timeouts kept");
        assert!(apply_fix("codex_sandbox".into(), None).unwrap().contains("无需修改"));
        assert_eq!(list_fixes().unwrap().len(), 3);

        // The user edits the file afterwards; undoing the sandbox fix keeps that edit.
        let edited = fs::read_to_string(&toml_p).unwrap().replace("model = \"gpt-6.1-sol\"", "model = \"gpt-6-astra\"");
        fs::write(&toml_p, &edited).unwrap();
        let sandbox_id = list_fixes().unwrap().into_iter().find(|f| f.id.ends_with("codex_sandbox")).unwrap().id;
        assert!(undo_fix(sandbox_id).unwrap().contains("只还原"));
        let doc: DocumentMut = fs::read_to_string(&toml_p).unwrap().parse().unwrap();
        assert!(doc.get("windows").is_none(), "empty [windows] dropped");
        assert_eq!(doc["model"].as_str(), Some("gpt-6-astra"));
        assert_eq!(doc["mcp_servers"]["fs"]["startup_timeout_sec"].as_integer(), Some(60));

        for f in list_fixes().unwrap() {
            undo_fix(f.id).unwrap();
        }
        assert!(!tmp.join(".codex/.env").exists());
        let doc: DocumentMut = fs::read_to_string(&toml_p).unwrap().parse().unwrap();
        assert!(doc["mcp_servers"]["fs"].get("startup_timeout_sec").is_none());
        assert_eq!(doc["model"].as_str(), Some("gpt-6-astra"));
        assert!(undo_fix("nope".into()).is_err());

        // proxy_clear only when something is set; broken JSON is never overwritten.
        assert!(apply_fix("proxy_clear".into(), None).is_err());
        fs::write(tmp.join(".claude/settings.json"), "{ broken").unwrap();
        assert!(apply_fix("claude_max_output".into(), None).is_err());
        assert_eq!(fs::read_to_string(tmp.join(".claude/settings.json")).unwrap(), "{ broken");
        assert!(apply_fix("bogus".into(), None).is_err());
        assert!(list_fixes().unwrap().is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn retries_and_chinese() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("retry");
        let toml_p = tmp.join(".codex/config.toml");
        // Official login: Codex forbids tuning the built-in provider.
        fs::write(&toml_p, TOML).unwrap();
        assert!(apply_fix("codex_retries".into(), None).is_err());
        let custom = "model_provider = \"xbtry\"\n\n[model_providers.xbtry]\nname = \"XBTry\"\nstream_max_retries = 3\n";
        fs::write(&toml_p, custom).unwrap();
        let mut out = vec![];
        codex_config_checks(&mut out);
        let c = out.iter().find(|c| c.title == "Codex 断线重连").unwrap();
        assert_eq!(c.fix.as_ref().unwrap().id, "codex_retries");
        assert!(diagnose("Reconnecting... 5/5".into()).iter().any(|d| d.fix.as_ref().is_some_and(|f| f.id == "codex_retries")));
        apply_fix("codex_retries".into(), None).unwrap();
        let doc: DocumentMut = fs::read_to_string(&toml_p).unwrap().parse().unwrap();
        assert_eq!(doc["model_providers"]["xbtry"]["stream_max_retries"].as_integer(), Some(20));
        assert_eq!(doc["model_providers"]["xbtry"]["request_max_retries"].as_integer(), Some(10));
        assert!(apply_fix("codex_retries".into(), None).unwrap().contains("无需修改"));
        undo_fix(list_fixes().unwrap()[0].id.clone()).unwrap();
        assert_eq!(fs::read_to_string(&toml_p).unwrap(), custom);

        // AGENTS.override.md wins when it has content; the user's text is kept.
        let ov = tmp.join(".codex/AGENTS.override.md");
        fs::write(&ov, "# mine\n").unwrap();
        apply_fix("codex_chinese".into(), None).unwrap();
        let t = fs::read_to_string(&ov).unwrap();
        assert!(t.starts_with("# mine\n") && t.contains("简体中文"), "{t}");
        assert!(!tmp.join(".codex/AGENTS.md").exists());
        assert!(apply_fix("codex_chinese".into(), None).unwrap().contains("无需修改"));
        fs::write(&ov, t.replace("# mine", "# mine edited")).unwrap();
        undo_fix(list_fixes().unwrap()[0].id.clone()).unwrap();
        assert_eq!(fs::read_to_string(&ov).unwrap(), "# mine edited\n");

        apply_fix("claude_chinese".into(), None).unwrap();
        let s: Value = serde_json::from_str(&fs::read_to_string(tmp.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(s["language"], "Simplified Chinese");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn chinese_ui() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("zhui");
        let toml_p = tmp.join(".codex/config.toml");
        fs::write(&toml_p, "[desktop]\nsansFontSize = 14\n").unwrap();
        let store = tmp.join("AppData/Local/Packages/Claude_test/LocalCache/Roaming/Claude");
        fs::create_dir_all(&store).unwrap();
        let cfg = "{\n  \"locale\": \"en-US\",\n  \"keep\": 1\n}\n";
        fs::write(store.join("config.json"), cfg).unwrap();
        let mut out = vec![];
        language_checks(&mut out);
        assert!(out.iter().any(|c| c.title == "Claude 桌面版界面语言" && c.fix.is_some()), "{out:?}");
        apply_fix("zh_all".into(), None).unwrap();
        let doc: DocumentMut = fs::read_to_string(&toml_p).unwrap().parse().unwrap();
        assert_eq!(doc["desktop"]["localeOverride"].as_str(), Some("zh-CN"));
        assert_eq!(doc["desktop"]["sansFontSize"].as_integer(), Some(14));
        let c: Value = serde_json::from_str(&fs::read_to_string(store.join("config.json")).unwrap()).unwrap();
        assert_eq!(c["locale"], "zh-Hans");
        assert_eq!(c["keep"], 1);
        assert!(fs::read_to_string(tmp.join(".codex/AGENTS.md")).unwrap().contains("简体中文"));
        undo_fix(list_fixes().unwrap()[0].id.clone()).unwrap();
        assert_eq!(fs::read_to_string(&toml_p).unwrap(), "[desktop]\nsansFontSize = 14\n");
        assert_eq!(fs::read_to_string(store.join("config.json")).unwrap(), cfg);
        assert!(!tmp.join(".codex/AGENTS.md").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn chinese_ui_gateway_mode() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("zh3p");
        let pkg = tmp.join("AppData/Local/Packages/AnthropicPBC.Claude_t/LocalCache");
        let (one, three) = (pkg.join("Roaming/Claude"), pkg.join("Local/Claude-3p"));
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&three).unwrap();
        fs::write(one.join("config.json"), "{\"locale\": \"zh-Hans\"}").unwrap();
        let orig3 = "{\"locale\": \"en-US\", \"keep\": 1}";
        fs::write(three.join("config.json"), orig3).unwrap();
        fs::write(three.join("claude_desktop_config.json"), "{\"deploymentMode\": \"3p\"}").unwrap();
        let mut out = vec![];
        language_checks(&mut out);
        let c = out.iter().find(|c| c.title == "Claude 桌面版界面语言").unwrap();
        assert!(c.fix.is_none() && c.detail.contains("Codex"), "gateway mode explains instead of offering a fix: {c:?}");
        apply_fix("claude_ui_zh".into(), None).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(three.join("config.json")).unwrap()).unwrap();
        assert_eq!(v["locale"], "zh-Hans");
        assert_eq!(v["keep"], 1);
        undo_fix(list_fixes().unwrap()[0].id.clone()).unwrap();
        assert_eq!(fs::read_to_string(three.join("config.json")).unwrap(), orig3);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ip_parsing() {
        let (ip, loc, asn, org) = parse_ping0("202.156.27.138\n新加坡 新加坡 新加坡\nAS4657\nStarHub Cable Vision Ltd\n").unwrap();
        assert_eq!(ip, "202.156.27.138");
        assert_eq!(loc.as_deref(), Some("新加坡 新加坡 新加坡"));
        assert_eq!(asn.as_deref(), Some("AS4657"));
        assert!(org.is_some());
        assert!(parse_ping0("<html>").is_none());
        assert!(!region_bad(Some("SG"), "新加坡"));
        assert!(region_bad(Some("HK"), ""));
        assert!(region_bad(None, "中国 广东 深圳"));
        assert!(!region_bad(Some("TW"), "中国 台湾"));
    }

    #[test]
    fn block_roundtrip() {
        let b = block_put("X\n", "zh", Some("v"));
        assert_eq!(b, "X\n\n<!-- xbtry:zh -->\nv\n<!-- /xbtry:zh -->\n");
        assert_eq!(block_get(&b, "zh").as_deref(), Some("v"));
        assert_eq!(block_put(&b, "zh", None), "X\n");
        assert_eq!(block_put(&block_put("", "zh", Some("v")), "zh", None), "");
        assert_eq!(block_get(&block_put(&b, "zh", Some("w")), "zh").as_deref(), Some("w"));
    }

    #[test]
    fn local_checks_flag_sandbox_and_mcp() {
        let _g = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = sandbox("checks");
        fs::write(tmp.join(".codex/config.toml"), format!("{TOML}\n[windows]\nsandbox = \"elevated\"\n")).unwrap();
        fs::write(tmp.join(".claude/settings.json"), "not json").unwrap();
        let mut out = vec![];
        codex_config_checks(&mut out);
        claude_config_checks(&mut out);
        let find = |t: &str| out.iter().find(|c| c.title == t).unwrap_or_else(|| panic!("{t} missing: {out:?}"));
        if cfg!(windows) {
            let sb = find("Codex 沙盒");
            assert_eq!(sb.level, Level::Warn);
            assert_eq!(sb.fix.as_ref().unwrap().id, "codex_sandbox");
        }
        assert!(find("Codex MCP").detail.contains("fs"));
        assert!(!find("Codex MCP").detail.contains("slow"));
        assert_eq!(find("Claude Code 配置").level, Level::Err);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn diagnose_matches_known_errors() {
        let ids = |t: &str| diagnose(t.into()).into_iter().map(|d| d.fix.map(|f| f.id).unwrap_or_else(|| d.title.into())).collect::<Vec<_>>();
        assert_eq!(ids("Error: setup refresh had errors"), ["codex_sandbox"]);
        assert_eq!(ids("MCP client for `fs` timed out after 10 seconds"), ["codex_mcp_timeout"]);
        assert_eq!(ids("Claude's response exceeded the 32000 output token maximum"), ["claude_max_output"]);
        assert_eq!(ids("API Error: 401 {\"type\":\"authentication_error\"}"), ["密钥无效"]);
        assert_eq!(ids("connect ECONNREFUSED 127.0.0.1:7890"), ["proxy_clear"]);
        assert!(diagnose("   ".into()).is_empty());
        assert!(diagnose("hello world".into()).is_empty());
    }

    #[test]
    fn diagnose_new_rules() {
        let ids = |t: &str| diagnose(t.into()).into_iter().map(|d| d.fix.map(|f| f.id).unwrap_or_else(|| d.title.into())).collect::<Vec<_>>();
        assert!(ids("Selected model is at capacity. Please try a different model.").contains(&"模型满载".to_string()));
        assert_eq!(ids("ChatGPT 需要一次性权限才能在你的电脑上运行"), ["codex_acl_reset"]);
        assert!(ids("请把 session 粘贴到这里").contains(&"当心盗号".to_string()));
        let d = diagnose("stream disconnected before completion".into());
        assert_eq!(d[0].title, "连接中断", "proxy advice comes first");
    }

    #[test]
    fn helpers() {
        assert_eq!(valid_proxy(" http://127.0.0.1:7897/ ").unwrap(), "http://127.0.0.1:7897");
        assert!(valid_proxy("127.0.0.1:7897").is_err());
        assert!(valid_proxy("http://u:p@127.0.0.1:7897").is_err());
        assert!(valid_proxy("http://127.0.0.1:99999").is_err());
        assert!(valid_proxy("http://a b:1").is_err());
        assert_eq!(local_port("http://127.0.0.1:7897"), Some(7897));
        assert_eq!(local_port("http://proxy.corp:8080"), None);
        assert_eq!(env_put("A=1\n# B=2\nB=3\nB=4\n", "B", Some("9")), "A=1\n# B=2\nB=9\n");
        assert_eq!(env_put("A=1\n", "A", None), "");
        assert_eq!(env_get("export HTTPS_PROXY=\"x\"\n", "HTTPS_PROXY").as_deref(), Some("x"));
        assert!(valid_version("2.1.311") && valid_version("stable") && !valid_version("2.1") && !valid_version("1.2.3; rm"));
        assert!(terminal_script("claude_install_version", Some("2.1.311 & calc")).is_err());
        assert!(terminal_script("whoami", None).is_err());
        assert!(conflicting("anthropic_api_key") && conflicting("HTTPS_PROXY") && !conflicting("PATH"));
    }
}