//! Live exit-IP monitor. Records the exit IP at start, checks it every N
//! seconds, and force-quits Claude as soon as it changes. While the IP stays
//! different, a relaunched Claude is closed again; it unlocks when the IP is back.

use crate::config::{env_dir, home, R};
use crate::doctor::{now_ms, system_proxy};
use serde::Serialize;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Sites that echo the caller's IP. The Cloudflare traces on claude.ai and
/// api.anthropic.com show the IP Claude itself sees. Rotating them keeps a
/// 1-second interval at one request per site every 5 seconds.
const IP_ECHO: [(&str, bool); 5] = [
    ("https://claude.ai/cdn-cgi/trace", true),
    ("https://api.ipify.org", false),
    ("https://api.anthropic.com/cdn-cgi/trace", true),
    ("https://ipinfo.io/ip", false),
    ("https://ping0.cc/ip", false),
];
/// Consecutive failed lookups before Claude is stopped (proxy or network died).
const MAX_FAILS: u32 = 3;

pub(crate) fn parse_ip(text: &str, trace: bool) -> Option<String> {
    let raw = if trace {
        text.lines().find_map(|l| l.trim().strip_prefix("ip="))?
    } else {
        text.lines().map(str::trim).find(|l| !l.is_empty())?
    };
    let ok = raw.len() <= 45
        && (raw.contains('.') || raw.contains(':'))
        && raw.chars().all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':');
    ok.then(|| raw.to_string())
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct MonitorState {
    running: bool,
    /// Claude was stopped and the IP is still different.
    tripped: bool,
    interval: u64,
    route: String,
    baseline: Vec<String>,
    last_ip: Option<String>,
    last_at: u64,
    checks: u64,
    fails: u32,
    kill_cli: bool,
    alert: Option<String>,
    alert_at: u64,
}

static MON: LazyLock<Mutex<MonitorState>> = LazyLock::new(|| Mutex::new(MonitorState::default()));
/// Bumped on every start / stop; a loop with a stale generation exits.
static GEN: AtomicU64 = AtomicU64::new(0);

fn set(f: impl FnOnce(&mut MonitorState)) -> MonitorState {
    let mut g = MON.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut g);
    g.clone()
}
/// The route the desktop app uses: the system proxy when it is on, else direct.
fn desktop_route() -> Option<String> {
    match system_proxy() {
        (Some(s), true) => Some(s),
        _ => None,
    }
}

fn route_name(r: &Option<String>) -> String {
    r.as_ref().map_or_else(|| "直连".to_string(), |p| format!("系统代理 {p}"))
}

/// IPv4 only, so dual-stack sites never report a different (IPv6) address.
fn echo_client(route: &Option<String>, timeout: Duration) -> Option<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .user_agent("curl/8.0")
        .local_address(Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)));
    if let Some(p) = route {
        b = b.proxy(reqwest::Proxy::all(p).ok()?);
    }
    b.build().ok()
}

async fn echo_ip(c: &reqwest::Client, i: usize) -> Option<String> {
    let (url, trace) = IP_ECHO[i % IP_ECHO.len()];
    let res = c.get(url).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    parse_ip(&res.text().await.ok()?, trace)
}

/// Every address the echo sites report right now (a proxy may balance across a few).
async fn all_ips(c: &reqwest::Client) -> Vec<String> {
    let hs: Vec<_> = (0..IP_ECHO.len())
        .map(|i| {
            let c = c.clone();
            tauri::async_runtime::spawn(async move { echo_ip(&c, i).await })
        })
        .collect();
    let mut out = vec![];
    for h in hs {
        if let Ok(Some(ip)) = h.await {
            if !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

fn log(line: &str) {
    use std::io::Write;
    let Ok(dir) = home().map(|h| h.join(".xbtry")) else { return };
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("ip-monitor.log")) {
        let _ = writeln!(f, "{} {line}", now_ms());
    }
}
/// Desktop Claude lives under WindowsApps / AnthropicClaude / Programs\Claude.
/// The Claude Code CLI is also claude.exe, so it is matched separately.
pub(crate) fn is_desktop_path(p: &str) -> bool {
    let l = p.to_ascii_lowercase();
    l.contains("\\windowsapps\\") || l.contains("\\anthropicclaude\\") || l.contains("\\programs\\claude\\") || l.contains("/claude.app/")
}

#[cfg(windows)]
fn kill_claude(cli: bool) -> usize {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};
    let mut n = 0;
    // SAFETY: plain Win32 calls; every handle is closed, buffers are sized.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_ascii_lowercase();
            if name == "claude.exe" {
                let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                if !h.is_null() {
                    let mut buf = [0u16; 1024];
                    let mut size = buf.len() as u32;
                    let path = if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) != 0 {
                        String::from_utf16_lossy(&buf[..size as usize])
                    } else {
                        String::new()
                    };
                    if (is_desktop_path(&path) || cli) && TerminateProcess(h, 1) != 0 {
                        n += 1;
                    }
                    CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    n
}

#[cfg(target_os = "macos")]
fn kill_claude(cli: bool) -> usize {
    let mut n = 0;
    for (name, on) in [("Claude", true), ("claude", cli)] {
        let ok = on && std::process::Command::new("/usr/bin/pkill").args(["-9", "-x", name]).status().is_ok_and(|s| s.success());
        if ok {
            n += 1;
        }
    }
    n
}

#[cfg(not(any(windows, target_os = "macos")))]
fn kill_claude(_cli: bool) -> usize {
    0
}

/// XBTRY_HOME means sandbox / test mode: never touch the real Claude.
fn stop_claude(cli: bool) -> Option<usize> {
    if env_dir("XBTRY_HOME").is_some() { None } else { Some(kill_claude(cli)) }
}
fn publish(app: &tauri::AppHandle, s: &MonitorState) {
    use tauri::Emitter;
    let _ = app.emit("ip-monitor", s);
}

fn focus(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Stop Claude. The first trip raises the alert; later calls only close a
/// relaunched Claude while the IP is still wrong.
async fn trip(app: &tauri::AppHandle, gen: u64, why: String) {
    if GEN.load(Ordering::SeqCst) != gen {
        return;
    }
    let (first, cli) = {
        let g = MON.lock().unwrap_or_else(|e| e.into_inner());
        (!g.tripped, g.kill_cli)
    };
    let n = tauri::async_runtime::spawn_blocking(move || stop_claude(cli)).await.ok().flatten();
    if !first && n.unwrap_or(0) == 0 {
        return;
    }
    let msg = match (first, n) {
        (_, None) => format!("{why}（测试模式，没有结束进程）"),
        (true, Some(n)) => format!("{why}，已强制退出 Claude（{n} 个进程）"),
        (false, Some(n)) => format!("出口 IP 还没恢复，已再次关闭 Claude（{n} 个进程）"),
    };
    log(&msg);
    let s = set(|s| {
        s.tripped = true;
        s.alert = Some(msg);
        s.alert_at = now_ms();
    });
    publish(app, &s);
    if first {
        focus(app);
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Same,
    Changed,
    Fail,
}

fn classify(baseline: &[String], ip: &Option<String>) -> Outcome {
    match ip {
        Some(ip) if baseline.contains(ip) => Outcome::Same,
        Some(_) => Outcome::Changed,
        None => Outcome::Fail,
    }
}

/// Max lookups in flight; a slow site never delays the next tick.
const MAX_INFLIGHT: u64 = 4;
static INFLIGHT: AtomicU64 = AtomicU64::new(0);

async fn check_once(app: tauri::AppHandle, gen: u64, client: reqwest::Client, rname: String, i: usize) {
    let ip = echo_ip(&client, i).await;
    INFLIGHT.fetch_sub(1, Ordering::SeqCst);
    if GEN.load(Ordering::SeqCst) != gen {
        return;
    }
    let st = MON.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match classify(&st.baseline, &ip) {
        Outcome::Same => {
            let ip = ip.unwrap_or_default();
            let back = st.tripped;
            let s = set(|s| {
                s.fails = 0;
                s.checks += 1;
                s.last_ip = Some(ip.clone());
                s.last_at = now_ms();
                s.route = rname;
                if back {
                    s.tripped = false;
                    s.alert = Some(format!("出口 IP 已恢复为 {ip}，可以重新启动 Claude"));
                    s.alert_at = now_ms();
                }
            });
            if back {
                log(&format!("恢复 {ip}"));
            }
            publish(&app, &s);
        }
        Outcome::Changed => {
            let ip = ip.unwrap_or_default();
            let s = set(|s| {
                s.fails = 0;
                s.checks += 1;
                s.last_ip = Some(ip.clone());
                s.last_at = now_ms();
                s.route = rname;
            });
            publish(&app, &s);
            trip(&app, gen, format!("出口 IP 从 {} 变成 {ip}", st.baseline.join(" / "))).await;
        }
        Outcome::Fail => {
            let s = set(|s| {
                s.fails += 1;
                s.checks += 1;
                s.last_at = now_ms();
                s.route = rname;
            });
            publish(&app, &s);
            if s.fails >= MAX_FAILS {
                trip(&app, gen, format!("连续 {} 次查不到出口 IP（代理或网络断了）", s.fails)).await;
            }
        }
    }
}

async fn run(app: tauri::AppHandle, gen: u64, mut route: Option<String>, mut client: Option<reqwest::Client>) {
    let mut i = 0usize;
    loop {
        let t0 = Instant::now();
        if GEN.load(Ordering::SeqCst) != gen {
            return;
        }
        let interval = Duration::from_secs(MON.lock().unwrap_or_else(|e| e.into_inner()).interval.max(1));
        // Switching the system proxy on / off changes the desktop app's route: follow it.
        let now_route = desktop_route();
        if now_route != route {
            route = now_route;
            client = echo_client(&route, Duration::from_secs(4));
            log(&format!("线路变为 {}", route_name(&route)));
        }
        match &client {
            Some(c) if INFLIGHT.load(Ordering::SeqCst) < MAX_INFLIGHT => {
                INFLIGHT.fetch_add(1, Ordering::SeqCst);
                tauri::async_runtime::spawn(check_once(app.clone(), gen, c.clone(), route_name(&route), i));
                i += 1;
            }
            Some(_) => {}
            None => {
                INFLIGHT.fetch_add(1, Ordering::SeqCst);
                // Invalid proxy: counts as a failed lookup.
                let bad = reqwest::Client::builder().timeout(Duration::from_millis(1)).build().ok();
                if let Some(c) = bad {
                    tauri::async_runtime::spawn(check_once(app.clone(), gen, c, route_name(&route), i));
                }
            }
        }
        if let Some(rest) = interval.checked_sub(t0.elapsed()) {
            let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(rest)).await;
        }
    }
}

/// Read the exit IP(s) now, then watch every `interval` seconds (1-3600).
#[tauri::command]
pub async fn monitor_start(app: tauri::AppHandle, interval: u64, kill_cli: bool) -> R<MonitorState> {
    let interval = interval.clamp(1, 3600);
    let route = desktop_route();
    let client = echo_client(&route, Duration::from_secs(5)).ok_or("系统代理地址无效，无法开始监测")?;
    let baseline = all_ips(&client).await;
    if baseline.is_empty() {
        return Err(format!("通过{}查不到出口 IP，请先确认网络或代理正常", route_name(&route)));
    }
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let s = set(|s| {
        *s = MonitorState {
            running: true,
            interval,
            route: route_name(&route),
            baseline: baseline.clone(),
            last_ip: baseline.first().cloned(),
            last_at: now_ms(),
            kill_cli,
            ..Default::default()
        };
    });
    log(&format!("开始监测 {} 基准 {} 每 {interval} 秒", s.route, baseline.join(" / ")));
    let fast = echo_client(&route, Duration::from_secs(4));
    tauri::async_runtime::spawn(run(app, gen, route, fast));
    Ok(s)
}

#[tauri::command]
pub fn monitor_stop() -> MonitorState {
    GEN.fetch_add(1, Ordering::SeqCst);
    let s = set(|s| {
        s.running = false;
        s.tripped = false;
    });
    log("停止监测");
    s
}

#[tauri::command]
pub fn monitor_status() -> MonitorState {
    MON.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Change the interval while running.
#[tauri::command]
pub fn monitor_interval(interval: u64) -> MonitorState {
    set(|s| s.interval = interval.clamp(1, 3600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_echo_answers() {
        assert_eq!(parse_ip("fl=1\nh=claude.ai\nip=202.156.27.138\nloc=SG\n", true).as_deref(), Some("202.156.27.138"));
        assert_eq!(parse_ip("202.156.27.138\n", false).as_deref(), Some("202.156.27.138"));
        assert_eq!(parse_ip("2001:db8::1", false).as_deref(), Some("2001:db8::1"));
        assert!(parse_ip("<html>blocked</html>", false).is_none());
        assert!(parse_ip("h=claude.ai\n", true).is_none());
        assert!(parse_ip("", false).is_none());
    }

    #[test]
    fn desktop_paths() {
        assert!(is_desktop_path(r"C:\Program Files\WindowsApps\Claude_4.2_x64__pzs8\app\Claude.exe"));
        assert!(is_desktop_path(r"C:\Users\a\AppData\Local\AnthropicClaude\app-1.0\claude.exe"));
        assert!(!is_desktop_path(r"C:\Users\a\.local\bin\claude.exe"));
        assert!(!is_desktop_path(r"C:\Users\a\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\claude.exe"));
    }

    #[test]
    fn classify_changes() {
        let b = vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()];
        assert_eq!(classify(&b, &Some("2.2.2.2".into())), Outcome::Same);
        assert_eq!(classify(&b, &Some("3.3.3.3".into())), Outcome::Changed);
        assert_eq!(classify(&b, &None), Outcome::Fail);
    }

    #[test]
    fn sandbox_never_kills() {
        let _g = crate::config::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("XBTRY_HOME", std::env::temp_dir());
        assert_eq!(stop_claude(true), None);
    }
}