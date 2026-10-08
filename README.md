# Agent Companion

[中文](#中文) · [English](#english) · [截图 / Screenshots](#截图--screenshots) · [下载 / Downloads](https://github.com/WXGopher/agent-companion/releases/latest)

## 中文

在 Windows 任务栏或 macOS 菜单栏查看 Codex 的任务状态与订阅用量。

### 主要功能

- **会话接力 CLI**：`acomp resume` 按当前目录搜索 Codex / Dodex 原会话，保留历史和设置，并选择本次使用的额度账号。通过 `agent-companion install-cli` 安装命令；macOS 需要受支持的原生 CLI 与登录方式，Windows 暂不支持恢复启动。详见[接力用法与兼容性](docs/RESUME.md)。
- **更新提示**：主动打开面板时在后台检查 GitHub 正式发布版本，每 24 小时最多检查一次，重启后仍复用本地记录。发现更高版本时显示版本号，点击“查看更新”打开对应 Release 页面。检查不阻塞面板；断网或检查失败时静默保留已有提示，不自动下载或安装。
- **任务状态**：查看运行中、待处理和已结束的任务，点击返回对应对话或终端。
- **Tasks / Usage**：切换任务与用量，查看剩余额度、重置时间、累计 Token 和最近七个有记录日期的用量柱状图。
- **状态栏定制**：选择 Codex CLI 状态栏组件，实时预览并保存。
- **Windows 集成**：任务栏分别显示 Codex（`C`）和 Dodex（`D`）的每周剩余额度；支持任务完成通知、开机启动，以及可选的工具审批和提问卡片。
- **macOS 入口**：菜单栏显示每周剩余额度，Codex 在上、Dodex 在下；有历史读数的实例在闲置时继续显示，仅最近一次额度查询失败时旧值带 `*`，悬停可查看最后成功时间和失败原因。尚无读数时显示 `—` 和对应任务状态；点击弹出任务／用量面板，设置从面板底部打开。
- **任务状态标记**：分别显示 Codex／Dodex 状态：蓝色呼吸表示进行中，黄色表示待确认／待输入或失败，绿色仅表示所有任务均完成，灰色表示无任务或停止、暂停、未知等状态。混合任务优先显示等待，其次显示运行。macOS 菜单栏与 Windows 任务栏使用相同颜色含义。
- **macOS 面板**：顶部显示 Agent Companion、当前版本和正在运行的任务数。点击底部的太阳／月亮按钮切换亮色／暗色主题，Tasks 和 Usage 同步更新；重启后保留主题选择，默认使用暗色。
- **TUI 双开（Windows / macOS）**：独立安装 Codex TUI 和 Dodex TUI，账号、配置、历史、数据库、日志、完整程序包、后台服务和更新目录分别管理，允许版本不同。首次运行 Dodex 时自行登录副账号；桌面只保留官方 Codex App。
- **配置与个人指令同步**：双开页签显示双方文件路径，提供独立的 Codex → Dodex、Dodex → Codex 操作，手动覆盖 `config.toml` 或各自 `CODEX_HOME` 下的全局 `AGENTS.md`，覆盖前备份目标。配置可能含有内嵌密钥，同步时会一并复制；**不复制 `auth.json`**，保留目标实例的登录存储、数据库和日志设置。没有自动同步，详见[同步说明](docs/macos-dual-instance.md#manual-configuration-and-instruction-sync)。

订阅用量复用本机 Codex TUI 已有的 ChatGPT 登录，无需另设 API Key。默认每 5 分钟自动刷新，设置可调整为 1–60 分钟；手动刷新只查询当前实例。额度与历史分别加载，查询不创建模型任务、不消耗推理 Token。Codex 的托管策略继续生效；无法确认账号或配置时显示原因。

界面保留同一账号最后成功的额度读数，查询失败后以 `*` 标注；尚无读数时显示 `—`。换号或登出会清除旧读数，额度重置时间到达后等待下次查询，不推算剩余额度。Usage 显示最后成功时间、失败原因和历史 Token 统计。详见[用量查询行为](docs/USAGE_QUERIES.md)。

### 安装

**Windows x86_64**

1. 安装 [Microsoft Visual C++ x64 运行库](https://aka.ms/vc14/vc_redist.x64.exe)。
2. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `windows-x86_64.zip` 结尾的压缩包，解压后双击 `agent-companion.exe`。
3. 如需工具审批，在解压目录打开 PowerShell，安装 Codex hooks：

```powershell
.\agent-companion.exe setup install codex
```

随后在 Codex 中用 `/hooks` 审阅并信任配置，再开启新会话。如需提问卡片及精确终端跳转，使用同目录的 `agent-companion-codex.exe` 启动 Codex。

**Windows TUI 双开**：右键任务栏读数或托盘图标 → **Settings… → TUI 双开 → 安装 Dodex TUI**。也可从解压目录运行：

```powershell
.\agent-companion.exe dodex-tui --install
.\agent-companion.exe dodex-tui --repair
dodex login
dodex resume
dodex exec "检查当前项目"
dodex -C C:\projects\demo
dodex update
```

不要求安装桌面 App。公开命令位于 `%LOCALAPPDATA%\AgentCompanion\bin`；新终端会读取新增的用户 PATH，旧版已登记的 Dodex 命令同时迁移。Dodex 默认账号目录是 `%LOCALAPPDATA%\AgentCompanion\Dodex\codex-home`，已有副账号目录继续原地使用。参数、工作目录、终端交互和退出码由所安装的原生 Codex 处理。`dodex update` 仅更新 Dodex；`dodex app` 明确提示桌面双开已移除。[双开说明](docs/macos-dual-instance.md)

**macOS 14+ · Apple Silicon**

1. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `macos-arm64.zip` 结尾的压缩包。
2. 解压，将 `Agent Companion.app` 拖到“应用程序”并打开。

应用尚未经过 Apple 公证；首次打开若被拦截，可按 [Apple 官方说明](https://support.apple.com/102445)在“系统设置 → 隐私与安全性”中选择“仍要打开”。

**macOS TUI 双开**：设置 → **TUI 双开 → 安装 Dodex TUI**。安装完整官方 standalone 包和独立终端入口；点击“打开终端”运行 Dodex 并自行登录副账号。命令入口为 `~/.local/bin/dodex`；也可运行 `agent-companion dodex-tui --install` 或 `--repair`。Codex 继续使用现有安装渠道，包括 Homebrew；不需要桌面 App。已有 Dodex 账号、数据库、会话及日志目录保留原位置。[双开说明](docs/macos-dual-instance.md)

**TUI 版本更新**：设置页只显示 Codex TUI 和 Dodex TUI，两行分别更新。打开页面只读取本机版本；更新由用户显式发起。`dodex update` 直接执行 Dodex 自身的原生更新，公开入口随 `current` 自动选择新版本。两套后台 socket、PID、锁和程序包分别属于各自的 `CODEX_HOME`；默认沿用原生后台机制，显式 `--no-daemon` 原样传递。

macOS 从菜单栏打开任务／用量面板，设置位于面板底部；重新打开 Agent Companion 会显示 Tasks 页。详见[菜单栏说明](docs/MACOS_MENU_BAR.md)。

**macOS 开机自动启动**：设置 → 通用 → 登录时自动启动。开启后在当前用户登录此 Mac 时启动菜单栏应用；关闭不退出当前应用。开关读取 macOS 实际登录项状态；如需系统批准或操作失败，会显示原因，并提供“系统登录项设置”入口。额度自动刷新间隔也在“通用”页。

**双开隔离与停用**：安装不会复制主账号认证、个人配置或 AGENTS 指令。高级设置保留显式配置同步；账号接力继续通过 `acomp resume` 操作。关闭监控不退出 TUI，也不删除账号数据。旧 App 部署记录只用于迁移，成功后归档；修复可重复运行，失败后保留原数据并报告原因。

**升级**：先退出旧程序及设置窗口，再替换文件；已安装 Windows hooks 的用户需重新执行安装命令。Codex 配置、账号和双开环境会保留。

## English

See Codex tasks and subscription usage in the Windows taskbar or macOS menu bar.

### Features

- **Session handoff CLI**: `acomp resume` finds original Codex / Dodex sessions in the current directory, preserves history and settings, and lets you choose the quota account for this run. Install commands with `agent-companion install-cli`. macOS requires a supported native CLI and login method; Windows resume launch is not supported. See [resume usage and compatibility](docs/RESUME.md).
- **Update notifications**: opening the panel checks for a newer stable GitHub release in the background, at most once every 24 hours across restarts. Updates link to their release page; the app does not download or install them automatically.
- **Tasks**: track running, waiting and finished tasks, then jump back to the conversation or terminal.
- **Tasks / Usage**: switch to remaining quota, reset times, lifetime tokens and a bar chart of the last seven reported days.
- **Status bar editor**: choose Codex CLI components with a live preview.
- **Windows integration**: separate taskbar readings for Codex (`C`) and Dodex (`D`) weekly quota remaining, completion notifications, startup settings, and optional tool approvals and question cards.
- **macOS menu bar**: the menu bar shows weekly quota remaining, with Codex above Dodex. Instances with a previous reading stay visible while idle; the previous value carries `*` only after a failed quota query, with the last success time and error on hover. Before a reading is available, its row shows `—` and the task status; click it for tasks and usage. The menu bar is the only macOS entry; open Settings from the popup footer.
- **Task status marks**: Codex / Dodex each show breathing blue for running tasks, yellow for approval/input or failure, green only when every task completed, and gray for no tasks, stopped, paused or unknown states. Waiting takes priority over running in mixed groups. The macOS menu bar and Windows taskbar use the same color meanings.
- **macOS panel**: the header shows Agent Companion, its current version and the number of running tasks. Use the sun / moon button at the bottom to switch light / dark themes across Tasks and Usage. Your theme choice is saved across restarts; dark is the default.
- **Two independent TUIs (Windows / macOS)**: Codex TUI and Dodex TUI keep separate accounts, configuration, history, databases, logs, complete packages, daemons and update directories. Versions can differ. Sign in to Dodex yourself; the official Codex App is the only supported desktop app.
- **Configuration and personal instructions**: the dual-instance tab shows both file paths and separate Codex → Dodex / Dodex → Codex actions for manually overwriting `config.toml` or the global `AGENTS.md` in each `CODEX_HOME`, with destination backups. Config files may contain embedded secrets, which are copied; **`auth.json` is never copied**, and the destination's account storage, database and log settings remain independent. Sync is never automatic. See [sync behavior](docs/macos-dual-instance.md#manual-configuration-and-instruction-sync).

Subscription usage reuses the existing ChatGPT login from your local Codex TUI, with no separate API key. Automatic refresh defaults to five minutes and can be set to 1–60 minutes; manual refresh queries only the selected instance. Quota and history load independently. Queries do not create model tasks or spend inference tokens. Codex's managed policies still apply, and unverifiable accounts or configurations show a reason.

The interface retains the last successful quota reading for the same account, marking it with `*` after a failed query and showing `—` before a reading is available. Switching accounts or signing out clears old readings. After a quota reset time, the app waits for the next query instead of estimating the remaining allowance. Usage shows the last success time, failure reason and historical token totals. See [usage query behavior](docs/USAGE_QUERIES.md).

### Install

**Windows x86_64**

1. Install the [Microsoft Visual C++ x64 Redistributable](https://aka.ms/vc14/vc_redist.x64.exe).
2. Download the archive ending in `windows-x86_64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest), extract it, and open `agent-companion.exe`.
3. For tool approvals, open PowerShell in the extracted folder and install Codex hooks:

```powershell
.\agent-companion.exe setup install codex
```

Use `/hooks` in Codex to review and trust the configuration, then start a new session. For question cards and precise terminal navigation, launch Codex through `agent-companion-codex.exe` in the same folder.

**Windows TUI instances**: Settings → **TUI 双开 → 安装 Dodex TUI**, or run `agent-companion.exe dodex-tui --install` from the extracted release. Use `--repair` to complete or repair an installation. The console entries live in `%LOCALAPPDATA%\AgentCompanion\bin`; reopen your terminal after PATH registration. Run `dodex login`, `dodex resume`, `dodex exec`, or `dodex update`. No desktop App is required. Existing secondary profiles remain in place. Arguments, terminal interaction, working directories and exit codes are native. `dodex app` reports that desktop duplication has been removed. [TUI guide](docs/macos-dual-instance.md)

**macOS 14+ · Apple Silicon**

1. Download the archive ending in `macos-arm64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest).
2. Extract it, drag `Agent Companion.app` into Applications, and open it.

The app is not Apple-notarized. If the first launch is blocked, follow [Apple's instructions](https://support.apple.com/102445) to choose **Open Anyway** in System Settings → Privacy & Security.

**macOS TUI instances**: Settings → **TUI 双开 → 安装 Dodex TUI** installs a complete official standalone package and `~/.local/bin/dodex`. Use **打开终端** to sign in with your second account. `agent-companion dodex-tui --install` and `--repair` run the same workflow. Codex keeps its existing channel, including Homebrew. Existing Dodex accounts, history, databases and logs remain at their original paths. [TUI guide](docs/macos-dual-instance.md)

**Independent updates**: Settings displays only Codex TUI and Dodex TUI, each with its own update button. Opening Settings observes local versions without contacting an update service. `dodex update` directly runs Dodex's native updater; its public entry follows the vendor's `current` selection. Each `CODEX_HOME` owns its sockets, PIDs, locks and background packages. Native daemon behavior is preserved; an explicit `--no-daemon` is forwarded unchanged.

On macOS, open tasks and usage from the menu bar, with Settings in the popup footer. Reopening Agent Companion shows the Tasks page. See the [menu-bar guide](docs/MACOS_MENU_BAR.md).

**macOS launch at login**: Settings → 通用 → 登录时自动启动 starts the menu-bar app when the current user logs in. Disabling it leaves the running app open. The switch reads the actual macOS login-item status; pending approval and failures remain visible with a link to System Settings. The quota refresh interval also lives in 通用.

**Isolation and disabling**: installation does not copy primary credentials, personal settings or AGENTS instructions. Advanced settings retain explicit configuration synchronization, and `acomp resume` retains account handoff. Disabling monitoring leaves the TUIs and their data usable. Desktop deployment records are used only for migration, then archived after successful publication. Repair is repeatable and preserves existing data after failures.

**Upgrade**: quit the old app and its Settings window, then replace the files. If you installed Windows hooks, rerun the installation command. Codex configuration, accounts and the second-instance environment are retained.

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

<p align="center"><img src="docs/macos-menu-bar.png" width="120" alt="macOS 菜单栏状态 / Menu-bar status"></p>
<p align="center">
  <img src="docs/macos-panel.png" width="356" alt="macOS 暗色任务面板 / Dark Tasks popup">
  <img src="docs/macos-panel-light.png" width="356" alt="macOS 亮色任务面板 / Light Tasks popup">
</p>

<details>
<summary>设置 / Settings</summary>

<p align="center"><img src="docs/macos-settings.png" width="760" alt="macOS 设置与状态栏预览 / Settings and status bar preview"></p>

</details>

---

[GPL-3.0-only](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)
