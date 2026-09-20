# Agent Companion

[中文](#中文) · [English](#english) · [截图 / Screenshots](#截图--screenshots) · [下载 / Downloads](https://github.com/WXGopher/agent-companion/releases/latest)

## 中文

在 Windows 任务栏或 macOS 菜单栏查看 Codex 的任务状态与订阅用量。

### 主要功能

- **任务状态**：查看运行中、待处理和已结束的任务，点击返回对应对话或终端。
- **Tasks / Usage**：切换任务与用量，查看剩余额度、重置时间、累计 Token 和最近七个有记录日期的用量柱状图。
- **状态栏定制**：选择 Codex CLI 状态栏组件，实时预览并保存。
- **Windows 集成**：任务栏分别显示 Codex（`C`）和 Dodex（`D`）的每周剩余额度；支持任务完成通知、开机启动，以及可选的工具审批和提问卡片。
- **macOS 入口**：菜单栏显示有效的每周剩余额度：两份额度有效时上行 Codex、下行 Dodex，仅一份有效时只显示该实例，均无有效额度时显示默认图标；点击弹出任务／用量面板。Dock 图标可选，点击打开设置。菜单栏、Dock 和刘海可在设置中独立开关，默认仅显示菜单栏，已有选择保持不变。
- **可选 Codex 双开（Windows / macOS）**：在设置中手动部署或接入 Dodex，任务标注来源，用量、缓存和 CLI 状态栏设置按实例管理。默认关闭，不自动接入或启动 Dodex。

订阅用量复用对应实例的 Codex CLI ChatGPT 登录，无需 API Key。两个平台的 Usage 查询结果均按实例缓存 5 分钟，从查询完成时计时；切换页面直接复用有效缓存，手动刷新可立即重新查询。已过重置时间的额度不会继续显示为有效值。

### 安装

**Windows x86_64**

1. 安装 [Microsoft Visual C++ x64 运行库](https://aka.ms/vc14/vc_redist.x64.exe)。
2. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `windows-x86_64.zip` 结尾的压缩包，解压后双击 `agent-companion.exe`。
3. 如需工具审批，在解压目录打开 PowerShell，安装 Codex hooks：

```powershell
.\agent-companion.exe setup install codex
```

随后在 Codex 中用 `/hooks` 审阅并信任配置，再开启新会话。如需提问卡片及精确终端跳转，使用同目录的 `agent-companion-codex.exe` 启动 Codex。

**Windows 双开**：右键任务栏读数或托盘图标 → **Settings… → Codex 双开 → 部署双开环境**。需要本机已安装且签名有效的官方 Microsoft Store Codex / ChatGPT 应用。部署完成后点击 **打开 Dodex**，自行登录第二个账号并完成 Windows 初始化。之后可直接右键任务栏读数或托盘图标，选择 **打开 Dodex**；也可在 Companion 解压目录运行：

```powershell
.\agent-companion.exe dodex
```

`dodex --check` 只检查官方运行程序；`dodex --deploy` 部署或校验环境，不打开窗口。独立环境位于 `%LOCALAPPDATA%\AgentCompanion\Dodex`。

**macOS 14+ · Apple Silicon**

1. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `macos-arm64.zip` 结尾的压缩包。
2. 解压，将 `Agent Companion.app` 拖到“应用程序”并打开。

应用尚未经过 Apple 公证；首次打开若被拦截，可按 [Apple 官方说明](https://support.apple.com/102445)在“系统设置 → 隐私与安全性”中选择“仍要打开”。

**macOS 双开**：设置 → Codex 双开 → 部署双开环境。需要本机已安装且签名有效的官方 Codex.app。完成后在“应用程序”中打开 Dodex 并登录。

macOS 即使隐藏所有入口，重新打开 Agent Companion 仍会进入设置。

**双开隔离与停用**：两个平台的新环境均使用独立文件凭证、历史、数据库和个人配置，不复制原实例的账号数据。已有环境只有通过兼容性和隔离校验才会接入，冲突时停止且不覆盖。停用只关闭 Companion 对第二实例的监控，不退出 Dodex、不删除环境。暂不提供双开运行程序的自动更新或卸载。

**升级**：先退出旧程序，再替换解压后的文件；已安装 Windows hooks 的用户需重新执行安装命令。已有配置会保留。

## English

See Codex tasks and subscription usage in the Windows taskbar or macOS menu bar.

### Features

- **Tasks**: track running, waiting and finished tasks, then jump back to the conversation or terminal.
- **Tasks / Usage**: switch to remaining quota, reset times, lifetime tokens and a bar chart of the last seven reported days.
- **Status bar editor**: choose Codex CLI components with a live preview.
- **Windows integration**: separate taskbar readings for Codex (`C`) and Dodex (`D`) weekly quota remaining, completion notifications, startup settings, and optional tool approvals and question cards.
- **macOS entry points**: the menu bar shows valid weekly quota readings, with Codex above Dodex when both are available, a single row when only one is available, and the default icon otherwise; click it for tasks and usage. The optional Dock icon opens Settings. Configure menu bar, Dock and notch visibility independently in Settings. Only the menu bar is on by default; saved choices are preserved.
- **Optional second Codex instance (Windows / macOS)**: explicitly deploy or connect Dodex in Settings. Tasks show their source; usage, caches and CLI status bar settings stay separate. Disabled by default, with no automatic adoption or launch.

Subscription usage uses each instance's Codex CLI ChatGPT login. No API key is needed. Both platforms cache Usage results separately for five minutes from completion; switching pages reuses a fresh result, while manual refresh reads again. Quota readings past their reset time are no longer treated as valid.

### Install

**Windows x86_64**

1. Install the [Microsoft Visual C++ x64 Redistributable](https://aka.ms/vc14/vc_redist.x64.exe).
2. Download the archive ending in `windows-x86_64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest), extract it, and open `agent-companion.exe`.
3. For tool approvals, open PowerShell in the extracted folder and install Codex hooks:

```powershell
.\agent-companion.exe setup install codex
```

Use `/hooks` in Codex to review and trust the configuration, then start a new session. For question cards and precise terminal navigation, launch Codex through `agent-companion-codex.exe` in the same folder.

**Windows second instance**: right-click the taskbar readout or tray icon → **Settings… → Codex 双开 → 部署双开环境**. Requires the locally installed, validly signed official Microsoft Store Codex / ChatGPT app. After deployment, click **打开 Dodex**, sign in with a second account and complete Windows setup. For later launches, right-click either the taskbar readout or tray icon and choose **打开 Dodex**, or run this from the Companion folder:

```powershell
.\agent-companion.exe dodex
```

`dodex --check` only checks the official runtime; `dodex --deploy` deploys or validates without opening a window. The isolated environment lives under `%LOCALAPPDATA%\AgentCompanion\Dodex`.

**macOS 14+ · Apple Silicon**

1. Download the archive ending in `macos-arm64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest).
2. Extract it, drag `Agent Companion.app` into Applications, and open it.

The app is not Apple-notarized. If the first launch is blocked, follow [Apple's instructions](https://support.apple.com/102445) to choose **Open Anyway** in System Settings → Privacy & Security.

**macOS second instance**: Settings → Codex 双开 → 部署双开环境. Requires a locally installed, validly signed official Codex.app. After deployment, open Dodex from Applications and sign in.

On macOS, reopening Agent Companion opens Settings even if all entry points are hidden.

**Isolation and disabling**: on both platforms, a fresh environment has separate file credentials, history, databases and personal settings, without copying account data from the original instance. Existing environments are adopted only after compatibility and isolation checks; conflicts stop without overwriting. Disabling integration stops Companion monitoring the second instance without quitting Dodex or deleting its environment. Automatic runtime updates and uninstall are not included.

**Upgrade**: quit the old app and replace its files. If you installed Windows hooks, rerun the installation command. Existing settings are retained.

## 截图 / Screenshots

界面使用演示数据。 / Screenshots use sample data.

### Windows

<p align="center">
  <img src="docs/panel.png" width="340" alt="Windows 任务列表 / Tasks">
  <img src="docs/windows-usage.png" width="340" alt="Windows Token 用量柱状图 / Usage">
</p>

<details>
<summary>设置 / Settings</summary>

<p align="center"><img src="docs/codex-tui.png" width="760" alt="Windows 分区设置与状态栏预览 / Settings and status bar preview"></p>

<p align="center"><img src="docs/windows-dual.png" width="760" alt="Windows Dodex 部署与启动 / Dodex deployment and launch"></p>

</details>

### macOS

<p align="center"><img src="docs/macos-notch.png" width="356" alt="macOS 刘海与任务列表 / Notch and tasks"></p>

<details>
<summary>设置 / Settings</summary>

<p align="center"><img src="docs/macos-settings.png" width="760" alt="macOS 设置与状态栏预览 / Settings and status bar preview"></p>

</details>

---

[GPL-3.0-only](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)
