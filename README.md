# XBTry 2.2

一键检测并修复 Claude Code / Codex 的常见问题，把 XBTry API Key 一键接入。Tauri 2 + 原生 HTML/CSS/JS，无前端构建步骤。

## 功能

- 一键检测：客户端和版本、配置文件、Codex 沙盒与 MCP 超时、Git Bash、冲突的环境变量、系统代理和本机代理端口、XBTry 中转站与密钥、OpenAI / Claude 官方状态
- 一键修复：只做核实过的修复，修复前备份，可在「恢复配置」里逐条撤回
- API 导入：4 步向导（客户端 → 密钥 → 模型 → 导入）
- 代理设置：自动找本机代理端口，写入 `~/.codex/.env` 和 Claude Code `settings.json` 的 env，也能一键清除；自定义根证书
- 报错诊断：粘贴报错，匹配原因并给出修复按钮
- 安装助手 / 版本管理：在终端里运行官方安装命令；Claude Code 关闭自动更新、切 stable、安装指定版本

## 开发

需要 Node.js、Rust 和 Windows 构建工具（WebView2）。

```bash
npm install
npm run dev     # 调试
npm run build   # 生成 NSIS 安装包
cd src-tauri && cargo test
```

## 写入的文件

- API 导入：Claude Code `~/.claude/settings.json`、`~/.claude.json`；Codex `~/.codex/config.toml`、`auth.json`、`xbtry-models.json`。首次导入前备份到 `~/.xbtry/backup/`。
- 一键修复：只改需要的键（`~/.codex/.env`、`config.toml` 的 `[windows] sandbox` / `startup_timeout_sec`、`settings.json` 的 env 等）。备份和记录在 `~/.xbtry/backup/fixes/`，与导入备份分开。

设置 `XBTRY_HOME` 可以把所有路径重定向到别的目录（测试用）。中转地址在 `src-tauri/src/config.rs` 顶部。
