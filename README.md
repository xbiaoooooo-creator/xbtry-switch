# XBTry 2.0

把 XBTry API Key 一键接入 Claude Code / Codex。Tauri 2 + 原生 HTML/CSS/JS，无前端构建步骤。

## 开发

需要 Node.js、Rust 和 Windows 构建工具（WebView2）。

```bash
npm install
npm run dev     # 调试
npm run build   # 生成 NSIS 安装包
cd src-tauri && cargo test
```

## 写入的文件

- Claude Code：`~/.claude/settings.json`（env 里的地址、Key、模型别名）、`~/.claude.json`（跳过首次登录）
- Codex：`~/.codex/config.toml`（`model_provider = "xbtry"`）、`auth.json`、`xbtry-models.json`（模型目录）

首次导入前备份到 `~/.xbtry/backup/`，界面里的「恢复原配置」会原样还原。中转地址在 `src-tauri/src/config.rs` 顶部。