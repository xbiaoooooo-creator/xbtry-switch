<div align="center">

# XBTry

### 一键导入 XBTry API 密钥，管理 Claude Code / Codex 供应商

**粘贴一次密钥，自动写入 Claude Code 与 Codex 配置 —— 不用再手改 JSON / TOML 配置文件。**

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](https://github.com/xbtry/xbtry-switch/releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)

官方网站：**[xbtry.com](https://xbtry.com)**

</div>

## 下载

到 [Releases](https://github.com/xbtry/xbtry-switch/releases/latest) 页面下载：

| 平台 | 文件 |
| --- | --- |
| Windows | `XBTry_x.x.x_x64-setup.exe`（NSIS）或 `XBTry_x.x.x_x64_en-US.msi` |
| macOS（Apple Silicon + Intel 通用） | `XBTry_x.x.x_universal.dmg` |

> macOS 未做公证签名时，首次打开请右键点 App →「打开」。

## 使用

1. 打开 XBTry 进入统一工作台，点击「接入密钥」或左侧的导入入口。
2. 粘贴你在 [xbtry.com](https://xbtry.com) 生成的 API Key（`sk-...`）。
3. 选择 Claude Code、Codex 或同时选择两个应用，点击「一键接入」。
4. 默认接入后立即启用；关闭开关时只保存配置。导入失败时可保留密钥重试失败的应用。

密钥更新后重复同一操作即可，会覆盖原有的 XBTry 条目而不是新增重复项。

## 1.1 界面更新

- 统一工作台，不再区分轻量模式和高级模式；旧偏好自动迁移。
- 参考官网的深咖 / 米白 / 暖橙配色、原版 XB 品牌 Logo、连接状态和快捷入口。
- 「更多应用」可展开，MCP、Skills、提示词、会话和统计都在固定导航。
- 较重页面按需加载；托盘节能功能更名「后台节能」。

## 开发

```bash
pnpm install
pnpm run dev
pnpm run build
```

本地生成未签名更新包时，可使用 `pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}'`；正式发布仍使用原有签名流程。

## 其他功能

- 多供应商管理：官方、第三方中转、自定义 endpoint，一键切换
- MCP / Skills / 提示词 / 会话管理
- 用量统计、路由与聚合模式
- 系统托盘快速切换

## 发布（维护者）

打 tag 即触发 GitHub Actions 同时构建 Windows MSI 与 macOS 通用 DMG：

```bash
git tag v1.0.1 && git push origin v1.0.1
```

需要在仓库 Secrets 里配置 `TAURI_SIGNING_PRIVATE_KEY`（ updater 签名私钥）。
macOS 如需 Developer ID 签名 + 公证，另配 `APPLE_*` 系列 secrets，详见
`.github/workflows/xbtry-release.yml` 头部注释。

## 致谢

基于 [CC Switch](https://github.com/farion1231/cc-switch)（MIT License）定制开发。
