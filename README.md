# Codex Quota

An animated, privacy-first Windows desktop viewer for local Codex usage limits, 5-hour quota, weekly quota, and reset countdowns.

[简体中文](#简体中文) · [Download for Windows](../../releases/latest) · [Build from source](#build-from-source)

> Codex Quota is an independent open-source community project. It is not affiliated with or endorsed by OpenAI.

## Why Codex Quota?

Codex shows usage information inside its interface, but a small always-available desktop window is easier to glance at while coding. Codex Quota reads local Codex session data and the local app-server connection. Your quota data stays on your computer.

## Features

- Live 5-hour and weekly/long-window quota percentages
- Accurate reset countdowns and observed timestamps
- Local app-server source with session-file fallback
- Windows 10/11 Acrylic transparency and glass theme
- Independent background and text opacity controls
- Fully responsive window from compact widget to large dashboard
- Smooth spring animations with a global animation switch
- English and Simplified Chinese interface
- Always-on-top pin button
- System tray, close-to-tray, autostart, and start-minimized options
- Local cache and detailed diagnostics
- No cloud account, telemetry, or remote quota service

## Download

Download the latest NSIS installer from [GitHub Releases](../../releases/latest).

Windows SmartScreen may warn about a new unsigned community application. Review the source and release checksum before installing.

## Privacy

Codex Quota reads only local Codex runtime/session information needed to display usage limits. It does not upload prompts, conversations, account details, or quota data.

## Build from source

Requirements:

- Windows 10 or Windows 11
- Node.js 20+
- Rust stable with the MSVC toolchain
- WebView2 Runtime

```powershell
npm install
npm run tauri build
```

The installer is generated under `src-tauri/target/release/bundle/nsis/`.

## Development

```powershell
npm install
npm run tauri dev
```

Quality checks:

```powershell
npm run build
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
```

## Contributing

Bug reports, Windows compatibility results, translations, accessibility improvements, and pull requests are welcome. Please include your Windows and Codex versions when reporting quota detection problems.

## License

[MIT](LICENSE)

---

## 简体中文

Codex Quota 是一款适用于 Windows 的开源 Codex 用量查看器，可显示 5 小时额度、每周额度和重置倒计时。

### 主要功能

- 实时显示 5 小时与每周/长期窗口剩余额度
- 显示准确的重置倒计时和观测时间
- 优先读取本地 App Server，失败时回退到本地会话文件
- Windows 10/11 Acrylic 透明毛玻璃效果
- 背景和文字透明度可分别调整
- 窗口内容可随尺寸连续缩放
- 完整动态效果及总开关
- 中文和英文界面
- 一键窗口置顶
- 系统托盘、开机启动、最小化启动和关闭到托盘
- 不上传提示词、对话、账户信息或额度数据

请前往 [GitHub Releases](../../releases/latest) 下载最新 Windows 安装包。

本项目是独立的开源社区项目，与 OpenAI 无隶属或官方认可关系。
