# Agent Companion

[中文](#中文) · [English](#english) · [截图 / Screenshots](#截图--screenshots) · [下载 / Downloads](https://github.com/WXGopher/agent-companion/releases/latest)

## 中文

在 Windows 任务栏或 macOS 菜单栏查看 Codex 的任务状态与订阅用量。

### v0.3.22 更新

- 统一两端额度刷新：默认每 5 分钟查询，设置可调整为 1–60 分钟；额度与历史 Token 统计独立加载。保留最后成功读数，仅查询失败时加 `*`，不在重置时间推算为 100%。
- 呼吸状态改为不透明的亮色与灰色渐变，每轮 2 秒；运行状态使用更醒目的亮蓝色，两端颜色含义保持一致。
- 统一设置页按钮、路径字段和字体，调整亮暗主题的中性色；macOS 设置使用苹方字体。
- 主动打开面板时在后台检查新版，每 24 小时最多一次，重启后保留检查记录；发现更新可直接打开对应 GitHub Release 页面。[发布说明](docs/releases/v0.3.22.md)

### v0.3.21 更新

- 修复长 TUI 会话的状态漏读：首次读取大日志时充分使用读取预算；已确认运行、且仍持有写入锁的任务不再因十五分钟无输出而消失。
- 无法判断写入锁时保留事件状态，避免数据库短暂不可读时误报任务断开；完成或空闲任务不会因进程存活而变为运行。
- macOS 兼容目录名为 `ChatGPT.app`、应用身份仍为 Codex 的官方安装，恢复打开应用、桌面跳转和运行程序发现。[发布说明](docs/releases/v0.3.21.md)

### 主要功能

- **更新提示**：主动打开面板时在后台检查 GitHub 正式发布版本，每 24 小时最多检查一次，重启后仍复用本地记录。发现更高版本时显示版本号，点击“查看更新”打开对应 Release 页面。检查不阻塞面板；断网或检查失败时静默保留已有提示，不自动下载或安装。
- **任务状态**：查看运行中、待处理和已结束的任务，点击返回对应对话或终端。
- **Tasks / Usage**：切换任务与用量，查看剩余额度、重置时间、累计 Token 和最近七个有记录日期的用量柱状图。
- **状态栏定制**：选择 Codex CLI 状态栏组件，实时预览并保存。
- **Windows 集成**：任务栏分别显示 Codex（`C`）和 Dodex（`D`）的每周剩余额度；支持任务完成通知、开机启动，以及可选的工具审批和提问卡片。
- **macOS 入口**：菜单栏显示每周剩余额度，Codex 在上、Dodex 在下；有历史读数的实例在闲置时继续显示，仅最近一次额度查询失败时旧值带 `*`，悬停可查看最后成功时间和失败原因。尚无读数时显示 `—` 和对应任务状态；点击弹出任务／用量面板。macOS 仅保留菜单栏入口，设置从弹出面板底部打开。
- **任务状态标记**：分别显示 Codex／Dodex 状态：蓝色呼吸表示进行中，黄色表示待确认／待输入或失败，绿色仅表示所有任务均完成，灰色表示无任务或停止、暂停、未知等状态。混合任务优先显示等待，其次显示运行；失败不会误报为完成。macOS 菜单栏与 Windows 任务栏使用相同颜色含义，Windows 保留现有图标形状。
- **macOS 面板**：顶部显示 Agent Companion、当前版本和正在运行的任务数。点击底部的太阳／月亮按钮切换亮色／暗色主题，Tasks 和 Usage 同步更新；重启后保留主题选择，默认使用暗色。
- **可选 Codex 双开（Windows / macOS）**：在设置中手动部署或接入 Dodex，任务标注来源，用量、缓存和 CLI 状态栏设置按实例管理。默认关闭，不自动接入或启动 Dodex。
- **配置与个人指令同步**：双开页签显示双方文件路径，提供独立的 Codex → Dodex、Dodex → Codex 操作，手动覆盖 `config.toml` 或各自 `CODEX_HOME` 下的全局 `AGENTS.md`，覆盖前备份目标。配置可能含有内嵌密钥，同步时会一并复制；**不复制 `auth.json`**，保留目标实例的登录存储、数据库和日志设置。没有自动同步，详见[同步说明](docs/macos-dual-instance.md#manual-configuration-and-instruction-sync)。

订阅用量复用对应实例的 Codex CLI ChatGPT 登录，无需 API Key。macOS 与 Windows 共用 Rust 查询服务：启动、定时到期和用户主动打开面板时查询所有已启用实例；手动刷新仅查询当前实例。默认每 5 分钟自动查询，设置中的“额度自动刷新间隔（分钟）”支持 1–60 分钟整数，所有实例共用。保存到应用配置目录的独立 `usage.json`，运行中生效并重新计时，不立即查询。

每次额度刷新只进行必要握手和一次 `account/rateLimits/read` 业务调用，20 秒超时，不预查登录、不附带历史查询、不创建模型任务、不消耗推理 Token；CLI 内部 HTTP 次数不作保证。结果到达即发布并结束临时进程。额度和历史 Token 统计各自独立，同一实例同类请求合并；成功或失败后均重新等待设定间隔，不额外重试。休眠恢复后最多补查一次。

切页、切实例、任务活动、界面绘制、额度重置和 Windows 悬停预览均不触发额度查询。历史 Token 统计仅在进入 Usage 页或切换该页实例时独立加载，成功和失败均从完成时缓存 5 分钟；停留页面不自动轮询，额度刷新按钮不刷新历史统计。关闭面板不取消额度请求。

普通数值始终表示最后成功查询值；仅最近一次额度查询失败后保留旧值并加 `*`，再次查询中保持该状态，成功后清除。首次无结果或成功响应未包含对应额度窗口时显示 `—`。时间经过或跨过重置时间不会加星号，也不会推算为 100%；详情会提示“已到重置时间，等待下次查询”。悬停与 Usage 页显示最后成功时间和失败原因。账号查询结果不落盘，本地会话日志不再更新 GUI 额度；Claude 和 headless 模式保持原行为。

### 安装

**Windows x86_64**

1. 安装 [Microsoft Visual C++ x64 运行库](https://aka.ms/vc14/vc_redist.x64.exe)。
2. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `windows-x86_64.zip` 结尾的压缩包，解压后双击 `agent-companion.exe`。
3. 如需工具审批，在解压目录打开 PowerShell，安装 Codex hooks：

```powershell
.\agent-companion.exe setup install codex
```

随后在 Codex 中用 `/hooks` 审阅并信任配置，再开启新会话。如需提问卡片及精确终端跳转，使用同目录的 `agent-companion-codex.exe` 启动 Codex。

**Windows 双开**：右键任务栏读数或托盘图标 → **Settings… → Codex 双开 → 启用 Dodex 支持**，也可点击 **部署双开环境 / 检查并接入**。需要本机已安装且签名有效的官方 Microsoft Store Codex / ChatGPT 应用。启用时先检查已有环境，缺少时自动部署，并配置 shell 的 `dodex` 命令；重复操作会校验环境、补齐命令，不覆盖已有账号和个人配置。在 PowerShell、cmd 或 Git Bash 中运行 `dodex`，会在当前终端和工作目录打开使用第二份配置的 Codex CLI；首次使用时自行登录第二个账号：

```powershell
dodex
dodex --help
dodex resume
dodex exec "检查当前项目"
dodex -C C:\projects\demo
```

普通参数直接传给 Codex CLI，`--help` 和 `--version` 也由 Codex 处理。仅以 `--check` 或 `--deploy` 开头时进入环境管理：`dodex --check` 只检查官方运行程序；`dodex --deploy` 部署或校验环境并修复 shell 支持。升级旧版命令时，从新版程序运行 `.\agent-companion.exe dodex --deploy`。独立环境位于 `%LOCALAPPDATA%\AgentCompanion\Dodex`，已有账号、会话和个人配置继续使用。

桌面版仍可通过任务栏或托盘菜单的 **打开 Dodex**，或 `.\agent-companion.exe dodex` 打开，使用同一份第二实例配置。

命令优先放入当前 PATH 已包含的用户命令目录，现有终端即可发现；Git Bash 如缓存了旧命令，可运行 `hash -r`。若需要新增 PATH 目录，设置会提示完全退出并重开终端。已有无关的同名命令不会被覆盖，设置会指出冲突路径。

**macOS 14+ · Apple Silicon**

1. 从[最新版本](https://github.com/WXGopher/agent-companion/releases/latest)下载以 `macos-arm64.zip` 结尾的压缩包。
2. 解压，将 `Agent Companion.app` 拖到“应用程序”并打开。

应用尚未经过 Apple 公证；首次打开若被拦截，可按 [Apple 官方说明](https://support.apple.com/102445)在“系统设置 → 隐私与安全性”中选择“仍要打开”。

**macOS 双开**：设置 → Codex 双开 → 安装 Dodex App。从本机签名有效的官方 `Codex.app` 或保留 Codex 身份的 `ChatGPT.app` 创建独立名称、图标和本地签名的公开 App。已有账号目录与 TUI 保持不变；新环境首次使用需登录。官方 App 更新后，下次启动 Dodex 会自动检查并同步；版本一致时直接启动，更新失败时继续使用现有版本。也可退出 Dodex 后点击“检查并同步”，或运行 `agent-companion dodex-app --sync`。详见 [Dodex App 说明](docs/macos-dual-instance.md)。

macOS 重新打开 Agent Companion 会显示弹窗的 Tasks 页。刘海、悬停展开、显示器选择与独立 Dock 入口已移除；旧版入口偏好不会隐藏菜单栏，已保存的菜单栏位置继续保留。设置从弹窗底部打开。详见[菜单栏说明](docs/MACOS_MENU_BAR.md)。

**双开隔离与停用**：两个平台的新环境均使用独立文件凭证、历史、数据库和个人配置，不复制原实例的账号数据。已有环境只有通过兼容性和隔离校验才会接入，冲突时停止且不覆盖。停用只关闭 Companion 对第二实例的监控，不退出 Dodex、不删除环境。暂不提供双开运行程序的自动更新或卸载。

**升级**：先退出旧程序及设置窗口，再替换文件；已安装 Windows hooks 的用户需重新执行安装命令。Codex 配置、账号和双开环境会保留。

## English

See Codex tasks and subscription usage in the Windows taskbar or macOS menu bar.

### New in v0.3.22

- Share quota refresh behavior across platforms: query every five minutes by default, configurable from 1–60 minutes, with token history loaded separately. Keep the last successful reading, add `*` only after a failed query, and never infer 100% remaining at a reset time.
- Use an opaque bright-to-gray breathing cycle lasting two seconds, with a brighter blue for running tasks and consistent color meanings across platforms.
- Align Settings buttons, path fields and typography, with neutral light/dark colors and PingFang on macOS.
- Check for updates in the background when you open the panel, at most once every 24 hours across restarts. Newer versions link directly to their GitHub Release page. [Release notes](docs/releases/v0.3.22.md)

### New in v0.3.21

- Improve long-running TUI detection: use the full initial log-read budget, and retain known active tasks with a held writer lock after fifteen minutes without output.
- Keep event state when writer liveness is unknown, avoiding false disconnections during temporary database failures. A live process alone cannot turn idle or completed work into running work.
- On macOS, recognize official Codex installations named `ChatGPT.app`, restoring application opening, desktop navigation and runtime discovery. [Release notes](docs/releases/v0.3.21.md)

### Features

- **Tasks**: track running, waiting and finished tasks, then jump back to the conversation or terminal.
- **Tasks / Usage**: switch to remaining quota, reset times, lifetime tokens and a bar chart of the last seven reported days.
- **Status bar editor**: choose Codex CLI components with a live preview.
- **Windows integration**: separate taskbar readings for Codex (`C`) and Dodex (`D`) weekly quota remaining, completion notifications, startup settings, and optional tool approvals and question cards.
- **macOS menu bar**: the menu bar shows weekly quota remaining, with Codex above Dodex. Instances with a previous reading stay visible while idle; the previous value carries `*` only after a failed quota query, with the last success time and error on hover. Before a reading is available, its row shows `—` and the task status; click it for tasks and usage. The menu bar is the only macOS entry; open Settings from the popup footer.
- **Task status marks**: Codex / Dodex each show breathing blue for running tasks, yellow for approval/input or failure, green only when every task completed, and gray for no tasks, stopped, paused or unknown states. Waiting takes priority over running in mixed groups; failures never count as successful completion. The macOS menu bar and Windows taskbar use the same color meanings; Windows keeps its existing icon shapes.
- **macOS panel**: the header shows Agent Companion, its current version and the number of running tasks. Use the sun / moon button at the bottom to switch light / dark themes across Tasks and Usage. Your theme choice is saved across restarts; dark is the default.
- **Optional second Codex instance (Windows / macOS)**: explicitly deploy or connect Dodex in Settings. Tasks show their source; usage, caches and CLI status bar settings stay separate. Disabled by default, with no automatic adoption or launch.
- **Configuration and personal instructions**: the dual-instance tab shows both file paths and separate Codex → Dodex / Dodex → Codex actions for manually overwriting `config.toml` or the global `AGENTS.md` in each `CODEX_HOME`, with destination backups. Config files may contain embedded secrets, which are copied; **`auth.json` is never copied**, and the destination's account storage, database and log settings remain independent. Sync is never automatic. See [sync behavior](docs/macos-dual-instance.md#manual-configuration-and-instruction-sync).

Subscription usage uses each instance's Codex CLI ChatGPT login, with no API key. macOS and Windows share one Rust service. Startup, a due timer, and actively opening the panel query every enabled instance; manual refresh queries only the selected instance. Automatic refresh defaults to five minutes. Settings accepts an integer from 1 to 60 minutes, shared by all instances and stored in the application configuration directory's separate `usage.json`. Saving restarts the interval without an immediate query.

Each quota refresh performs the required handshake and exactly one `account/rateLimits/read` business call, with a 20-second timeout. It does not precheck the login, fetch history, create a model task or spend inference tokens; this does not guarantee a single HTTP request inside the CLI. Quota is published as soon as it arrives and the temporary process ends. Quota and history have separate in-flight requests and state. Overlapping requests of the same type and instance merge. Success and failure both restart the interval, with no extra retries; waking from sleep triggers at most one overdue query.

Page or instance switches, task activity, rendering, quota resets and Windows hover previews do not query quota. Historical token statistics load independently only on entering Usage or switching its instance, with a separate five-minute cache from completion for both success and failure. Remaining on Usage does not poll history; the refresh button only refreshes quota. Closing the panel does not cancel quota requests.

Ordinary readings always show the last successful query. Only a failed quota query adds `*` to a retained value; querying again preserves that state until success clears it. Before the first result, or when a successful response omits a window, the value is `—`. Age and reset times never add a star or imply 100% remaining; after a reset the detail says it is waiting for the next query. Hover text and Usage show the last success time and failure reason. Account results are never persisted, and local session logs no longer feed GUI quota. Claude and headless behavior is unchanged.

Opening the full panel checks for a newer stable GitHub release in the background, at most once every 24 hours across restarts. A newer version shows its version number and a button to open its GitHub Release page. Checks do not delay the panel, and network failures quietly retain any known update. Hover previews do not check for updates; the app does not download or install them automatically.

### Install

**Windows x86_64**

1. Install the [Microsoft Visual C++ x64 Redistributable](https://aka.ms/vc14/vc_redist.x64.exe).
2. Download the archive ending in `windows-x86_64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest), extract it, and open `agent-companion.exe`.
3. For tool approvals, open PowerShell in the extracted folder and install Codex hooks:

```powershell
.\agent-companion.exe setup install codex
```

Use `/hooks` in Codex to review and trust the configuration, then start a new session. For question cards and precise terminal navigation, launch Codex through `agent-companion-codex.exe` in the same folder.

**Windows second instance**: right-click the taskbar readout or tray icon → **Settings… → Codex 双开 → 启用 Dodex 支持**, or click **部署双开环境 / 检查并接入**. Requires the locally installed, validly signed official Microsoft Store Codex / ChatGPT app. Enabling checks an existing environment, deploys one if missing, and registers the `dodex` shell command. Repeating this validates the deployment and repairs shell support without overwriting existing account data or personal settings. In PowerShell, cmd or Git Bash, `dodex` opens Codex CLI in the current terminal and working directory using the second profile. Sign in with a second account on first use:

```powershell
dodex
dodex --help
dodex resume
dodex exec "Inspect this project"
dodex -C C:\projects\demo
```

Regular arguments pass directly to Codex CLI, including `--help` and `--version`. Only a leading `--check` or `--deploy` selects deployment management: `dodex --check` checks the official runtime; `dodex --deploy` deploys or validates the environment and repairs shell support. To upgrade an older command, run `.\agent-companion.exe dodex --deploy` from the new release. The isolated environment lives under `%LOCALAPPDATA%\AgentCompanion\Dodex`; existing account data, sessions and personal settings stay in use.

For the desktop app, choose **打开 Dodex** from the taskbar or tray menu, or run `.\agent-companion.exe dodex`. It uses the same second-instance profile.

The command is installed in a supported user command directory already on the current PATH when possible, so existing terminals can find it. Run `hash -r` in Git Bash if it cached an older command. If a new PATH entry is needed, Settings asks you to fully quit and reopen the terminal. Unrelated commands with the same name are preserved, and Settings reports the conflicting path.

**macOS 14+ · Apple Silicon**

1. Download the archive ending in `macos-arm64.zip` from the [latest release](https://github.com/WXGopher/agent-companion/releases/latest).
2. Extract it, drag `Agent Companion.app` into Applications, and open it.

The app is not Apple-notarized. If the first launch is blocked, follow [Apple's instructions](https://support.apple.com/102445) to choose **Open Anyway** in System Settings → Privacy & Security.

**macOS second instance**: Settings → Codex 双开 → 安装 Dodex App. Creates a public App with its own name, icon and local signature from the validly signed official `Codex.app` or Codex-identified `ChatGPT.app` installed on this Mac. Existing profile directories and TUI stay unchanged; fresh environments require login. After the official App updates, the next Dodex startup checks and syncs it automatically. Matching versions launch directly; failed updates leave the existing App usable. Manual sync remains available after quitting Dodex through “检查并同步” or `agent-companion dodex-app --sync`. See the [Dodex App guide](docs/macos-dual-instance.md).

On macOS, reopening Agent Companion shows the popup's Tasks page. The notch, hover expansion, display selection and separate Dock entry have been removed. Legacy entry preferences cannot hide the menu bar, and its saved position is retained. Open Settings from the popup footer. See the [menu-bar guide](docs/MACOS_MENU_BAR.md).

**Isolation and disabling**: on both platforms, a fresh environment has separate file credentials, history, databases and personal settings, without copying account data from the original instance. Existing environments are adopted only after compatibility and isolation checks; conflicts stop without overwriting. Disabling integration stops Companion monitoring the second instance without quitting Dodex or deleting its environment. Automatic runtime updates and uninstall are not included.

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
