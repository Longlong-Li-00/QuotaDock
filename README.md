# QuotaDock

在 Windows 上随时查看 **Codex** 和 **Claude** 订阅额度的小工具：主窗口、桌面小浮窗、托盘图标和 Windows 通知。

**只读、本地：** 不登录你的账号，不读取令牌，不上传任何数据，不给 AI 发提示词。

![概览](docs/images/app-overview.png)

| 桌面小浮窗（圆环） | 托盘菜单 |
|---|---|
| ![浮窗](docs/images/widget-detailed.png) | ![托盘菜单](docs/images/tray-menu.png) |

## 快速开始

1. 电脑上装好并登录 **Codex** 或 **Claude Code**（一个就行）。
2. 安装 QuotaDock：
   - 有安装包：到 Releases 下载 `QuotaDock_1.1.0-rc.2_x64-setup.exe`，双击，一路“下一步”。
   - 没有安装包：按手册里的“从源码构建”自己打包。
3. 打开后看 **概览** 页；要桌面浮窗，到 设置 → 悬浮窗 打开。

**详细步骤、每个页面的说明、提醒设置、Claude 的数据来源、常见问题，请看 [使用手册（中文）](docs/MANUAL.md)。** 也可以下载 [PDF 版手册](docs/QuotaDock-Manual.zh-CN.pdf)（24 页，含全部截图）。

## 系统要求

- Windows 11（64 位），不需要管理员权限
- Codex 官方桌面版或命令行，和/或 Claude Code（订阅账号登录）

## 当前状态

- 版本：**1.1.0-rc.2**（候选版本）
- 安装包 **还没有代码签名**，Windows 可能提示“未知发布者”
- 开发过程和验收记录在仓库根目录的 `*_FINDINGS.md` 文件里

## English (short)

QuotaDock is a read-only, local-first Windows 11 app that shows your remaining **Codex** and **Claude** subscription quota (5-hour and weekly windows) in a main window, a desktop widget, a tray icon and Windows notifications. It never logs in for you, never reads tokens, never uploads data, and never sends prompts to a model. The UI has English and Chinese; the full manual ([docs/MANUAL.md](docs/MANUAL.md)) is currently in Chinese only. To build from source you need Rust (MSVC), Node.js 23.6+, and then `powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1` (see the manual for the one-time tool setup). The installer is not code-signed yet.
