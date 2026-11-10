//! XBTry account inside the app. Uses the same user database and endpoints as
//! api.xbtry.com (captcha, email code, register with invite code, login).
//! The session token and the account's API keys are stored on this machine
//! only, encrypted with Windows DPAPI (0600 file elsewhere).

use crate::config::{home, R, CLAUDE_BASE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const COOKIE: &str = "xbtry_session";

#[derive(Serialize, Deserialize, Default, Clone)]
pub(crate) struct Saved {
    pub token: String,
    pub email: String,
    #[serde(default)]
    pub gpt_key: Option<String>,
    #[serde(default)]
    pub claude_key: Option<String>,
}

fn store_path() -> R<PathBuf> {
    Ok(home()?.join(".xbtry").join("account.dat"))
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    // SAFETY: input blob points at `data`; the output buffer is freed with LocalFree.
    unsafe {
        let inb = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = if encrypt {
            CryptProtectData(&inb, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&inb, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as _);
        Some(v)
    }
}

#[cfg(not(windows))]
fn dpapi(data: &[u8], _encrypt: bool) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

pub(crate) fn load() -> Option<Saved> {
    let raw = fs::read(store_path().ok()?).ok()?;
    serde_json::from_slice(&dpapi(&raw, false)?).ok()
}

fn save(s: &Saved) -> R<()> {
    let p = store_path()?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let plain = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    let enc = dpapi(&plain, true).ok_or("无法加密保存登录信息")?;
    fs::write(&p, enc).map_err(|e| format!("保存登录信息失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn clear() {
    if let Ok(p) = store_path() {
        let _ = fs::remove_file(p);
    }
}

/// GPT-pool key of the signed-in account, used for AI checks.
pub(crate) fn gpt_key() -> Option<String> {
    load()?.gpt_key.filter(|k| !k.is_empty())
}
fn client() -> R<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("XBTry-App/2.3")
        .build()
        .map_err(|e| e.to_string())
}

/// Call the site API. Returns the JSON body and a new session token if one was set.
async fn call(post: bool, path: &str, token: Option<&str>, body: Option<Value>) -> R<(Value, Option<String>)> {
    let c = client()?;
    let url = format!("{CLAUDE_BASE}{path}");
    let mut req = if post { c.post(url).json(&body.unwrap_or(json!({}))) } else { c.get(url) };
    if let Some(t) = token {
        req = req.header("Cookie", format!("{COOKIE}={t}"));
    }
    let res = req.send().await.map_err(|_| "连不上 api.xbtry.com，请检查网络".to_string())?;
    let status = res.status();
    let new_token = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.strip_prefix(&format!("{COOKIE}=")))
        .map(|v| v.split(';').next().unwrap_or("").to_string())
        .find(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit()));
    let j: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = j["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("请求失败（HTTP {}）", status.as_u16()));
        return Err(if status.as_u16() == 401 && path != "/api/auth/login" { "登录已过期，请重新登录".into() } else { msg });
    }
    Ok((j, new_token))
}

#[derive(Serialize)]
pub struct Captcha {
    id: String,
    image: String,
}

#[tauri::command]
pub async fn account_captcha() -> R<Captcha> {
    let (j, _) = call(false, "/api/auth/captcha", None, None).await?;
    Ok(Captcha {
        id: j["captchaId"].as_str().unwrap_or_default().to_string(),
        image: j["image"].as_str().unwrap_or_default().to_string(),
    })
}

fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 254 && !e.contains(char::is_whitespace) && e.split_once('@').is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.starts_with('.') && !b.ends_with('.'))
}

/// Send the email code. purpose: "register" or "login". Returns the challenge id.
#[tauri::command]
pub async fn account_send_code(email: String, purpose: String, captcha_id: String, captcha: String) -> R<String> {
    if !valid_email(&email) {
        return Err("邮箱格式不对，请检查一下".into());
    }
    if purpose != "register" && purpose != "login" {
        return Err("验证码用途不正确".into());
    }
    let body = json!({ "channel": "email", "account": email.trim(), "purpose": purpose, "captchaId": captcha_id, "captcha": captcha.trim() });
    let (j, _) = call(true, "/api/auth/send-code", None, Some(body)).await?;
    j["challengeId"].as_str().map(str::to_string).ok_or_else(|| "验证码发送失败，请重试".into())
}

#[derive(Deserialize)]
pub struct AuthReq {
    mode: String,
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    challenge_id: String,
    #[serde(default)]
    invite: String,
}

#[derive(Serialize, Default)]
pub struct AccountInfo {
    signed_in: bool,
    email: String,
    balance_cents: i64,
    invite_code: String,
    has_gpt_key: bool,
    has_claude_key: bool,
    /// Masked GPT key, e.g. sk-…1234.
    gpt_hint: Option<String>,
}

fn hint(k: &Option<String>) -> Option<String> {
    k.as_ref().map(|k| format!("{}…{}", &k[..k.len().min(3)], &k[k.len().saturating_sub(4)..]))
}

fn info(user: &Value, s: &Saved) -> AccountInfo {
    AccountInfo {
        signed_in: true,
        email: user["email"].as_str().map(str::to_string).filter(|e| !e.is_empty()).unwrap_or_else(|| s.email.clone()),
        balance_cents: user["balanceCents"].as_i64().unwrap_or(0),
        invite_code: user["inviteCode"].as_str().unwrap_or_default().to_string(),
        has_gpt_key: s.gpt_key.is_some(),
        has_claude_key: s.claude_key.is_some(),
        gpt_hint: hint(&s.gpt_key),
    }
}

/// Open the relay account (creates the Claude + GPT pool keys) and keep the keys.
async fn fetch_keys(s: &mut Saved) -> R<()> {
    let _ = call(true, "/api/relay/activate", Some(&s.token), None).await?;
    let (st, _) = call(false, "/api/relay/status", Some(&s.token), None).await?;
    let keys = st["keys"].as_array().cloned().unwrap_or_default();
    let is_gpt = |k: &Value| k["group"].as_i64() == Some(3) || k["group"].as_str() == Some("gpt") || k["group"].as_str() == Some("3");
    let pick = |gpt: bool| {
        keys.iter()
            .filter(|k| is_gpt(k) == gpt)
            .filter_map(|k| k["key"].as_str())
            .find(|k| k.starts_with("sk-"))
            .map(str::to_string)
    };
    s.gpt_key = pick(true);
    s.claude_key = pick(false);
    if s.gpt_key.is_none() {
        // Older accounts only have a Claude-pool key; AI checks need the GPT pool.
        let (j, _) = call(true, "/api/relay/keys", Some(&s.token), Some(json!({ "name": "XBTry AI", "group": "gpt" }))).await?;
        s.gpt_key = j["key"]["key"].as_str().filter(|k| k.starts_with("sk-")).map(str::to_string);
    }
    Ok(())
}

#[tauri::command]
pub async fn account_auth(req: AuthReq) -> R<AccountInfo> {
    let email = req.email.trim().to_string();
    if !valid_email(&email) {
        return Err("邮箱格式不对，请检查一下".into());
    }
    let pw_needed = req.mode != "code";
    if pw_needed && !(8..=128).contains(&req.password.chars().count()) {
        return Err("密码需要 8–128 位".into());
    }
    if req.mode != "password" && !(req.code.len() == 6 && req.code.chars().all(|c| c.is_ascii_digit())) {
        return Err("请输入 6 位邮箱验证码".into());
    }
    let (path, body) = match req.mode.as_str() {
        "register" => (
            "/api/auth/register",
            json!({ "channel": "email", "account": email, "password": req.password, "challengeId": req.challenge_id,
                    "code": req.code.trim(), "inviteCode": req.invite.trim().to_uppercase(), "fingerprint": fingerprint(), "remember": true }),
        ),
        "code" => ("/api/auth/login", json!({ "account": email, "method": "code", "channel": "email", "challengeId": req.challenge_id, "code": req.code.trim(), "remember": true })),
        "password" => ("/api/auth/login", json!({ "account": email, "password": req.password, "method": "password", "channel": "email", "remember": true })),
        _ => return Err("登录方式不正确".into()),
    };
    let (j, token) = call(true, path, None, Some(body)).await?;
    let token = token.ok_or("登录成功但没有拿到会话，请重试")?;
    let mut s = Saved { token, email: email.clone(), ..Default::default() };
    // Keys are optional at this point: a failure here must not lose the login.
    let key_err = fetch_keys(&mut s).await.err();
    save(&s)?;
    let user = match call(false, "/api/auth/me", Some(&s.token), None).await {
        Ok((me, _)) => me["user"].clone(),
        Err(_) => j["user"].clone(),
    };
    let out = info(&user, &s);
    if let Some(e) = key_err {
        if out.balance_cents <= 0 {
            return Ok(out);
        }
        return Err(format!("已登录，但开通 API 密钥失败：{e}"));
    }
    Ok(out)
}

/// Current account, refreshed from the site. Also fills in missing keys.
#[tauri::command]
pub async fn account_me() -> R<AccountInfo> {
    let Some(mut s) = load() else { return Ok(AccountInfo::default()) };
    let (me, _) = match call(false, "/api/auth/me", Some(&s.token), None).await {
        Ok(v) => v,
        Err(e) if e.contains("过期") => {
            clear();
            return Ok(AccountInfo::default());
        }
        Err(e) => return Err(e),
    };
    if me["user"].is_null() {
        clear();
        return Ok(AccountInfo::default());
    }
    if s.gpt_key.is_none() && fetch_keys(&mut s).await.is_ok() {
        let _ = save(&s);
    }
    Ok(info(&me["user"], &s))
}

#[tauri::command]
pub async fn account_logout() -> R<()> {
    if let Some(s) = load() {
        let _ = call(true, "/api/auth/logout", Some(&s.token), None).await;
    }
    clear();
    Ok(())
}

/// Keys of the signed-in account for the API import wizard (only on explicit request).
#[tauri::command]
pub fn account_keys() -> R<Value> {
    let s = load().ok_or("请先登录")?;
    Ok(json!({ "gpt": s.gpt_key, "claude": s.claude_key }))
}

/// Stable per-machine fingerprint, same idea as the website's device check.
fn fingerprint() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in ["COMPUTERNAME", "USERNAME", "PROCESSOR_IDENTIFIER", "NUMBER_OF_PROCESSORS", "HOSTNAME", "USER"] {
        std::env::var(v).unwrap_or_default().hash(&mut h);
    }
    format!("app{:x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_is_encrypted() {
        let _g = crate::config::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("xbtry-acct-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        std::env::set_var("XBTRY_HOME", &tmp);
        let s = Saved { token: "a".repeat(64), email: "a@b.co".into(), gpt_key: Some("sk-secret-1234".into()), claude_key: None };
        save(&s).unwrap();
        let raw = fs::read(store_path().unwrap()).unwrap();
        if cfg!(windows) {
            assert!(!String::from_utf8_lossy(&raw).contains("sk-secret"), "key must not be stored in clear text");
        }
        assert_eq!(gpt_key().as_deref(), Some("sk-secret-1234"));
        clear();
        assert!(load().is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn email_and_hint() {
        assert!(valid_email("a@b.co") && !valid_email("a b@c.d") && !valid_email("abc") && !valid_email("a@b."));
        assert_eq!(hint(&Some("sk-abcdef1234".into())).as_deref(), Some("sk-…1234"));
    }
}