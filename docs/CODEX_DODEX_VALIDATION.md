# Codex / Dodex 修复与验收记录

> Historical validation of the removed App duplication model. Current TUI-only behavior and acceptance are documented in [TUI instances](macos-dual-instance.md).

本轮验收日期：2026-10-02 至 2026-10-03。验收分为 `Codex App = Dodex App` 与
`Codex TUI = Dodex TUI` 两组；不要求 App 与 TUI 使用同一版本号。

## 执行边界

- 不退出、重启或替换现存 Codex TUI 及其资源，不修改其登录状态。
- 自动测试只使用临时目录、合成凭据、测试进程和本地服务。
- 自动测试不连接或清理用户现有 daemon，不运行实际账号额度查询；用户授权的
  Companion 安装后正常监控观察与真实账号接力启动验收单独记录。
- 本地构建、交叉编译和打包不等于部署完成；Windows 交叉编译不等于 Windows 实机验收。
- 真实账号读取可能触发原生客户端自动刷新凭据，因此集中到最终维护窗口。
  参见[官方认证说明](https://learn.chatgpt.com/docs/auth)。

## 验收证据

以下为本轮实际执行结果。不同测试目标会重复导入部分模块，不把各行相加作为独立用例总数。

| 检查 | 实际结果 |
| --- | --- |
| 核心 `--all-features` | 267 项通过，2 项原有忽略（包含安装后兼容回归） |
| 主程序 `--all-features` | 160 项通过（包含安装后兼容回归） |
| `acomp` 单元测试 | 9 项通过 |
| 便携 `windows_settings` 测试目标（在 macOS 执行） | 146 项通过；含导入的共有模块测试 |
| 其他集成测试 | resume_command 5、windows_indicators 9、windows_updates 1、windows_usage 25 项通过 |
| Python manager 测试 | 24 项通过 |
| Python scripts/tests | 55 项通过，12 项原生测试默认跳过后单独执行 |
| 原生 Codex 0.159.3 + 最新调试 `acomp` 合成账号契约 | 12 项全部通过，无原生项遗留跳过 |
| 旧 Dodex 0.155.0-alpha.16.4 与 Codex 0.159.3 共用 worker 契约 | 两版各 1 项通过：额度先于慢历史返回、配置核验、合成凭据及 daemon 哨兵保持不变 |
| 托管 Dodex wrapper 的原生 session/resume | 通过 |
| `acomp` PTY 交互与退出 | 通过 |
| Swift 终端恢复命令环境隔离 | 先复现失败再修复通过；真实 shell 子进程验证 14 类覆盖变量清理及沙箱/网络/代理保留 |
| Slint/AppKit 真实编辑器 | 34 个阶段通过；测试显式请求重绘以覆盖锁屏后的后台帧调度 |
| Swift 原生 UI 解锁后完整复测 | 10 个原生套件全部通过；兼容修复后的最终 Rust 验收目标 1 passed / 0 failed，耗时 140.80 秒 |
| macOS workspace 全目标 Clippy `-D warnings` | 通过 |
| Windows GNU workspace 全目标 Clippy `-D warnings` | 通过；包含 Windows 专属测试的编译检查 |
| macOS release 两个应用/终端入口 | 构建通过 |
| Windows GNU 四个 PE 可执行文件 | 编译、链接通过；未在 Windows 执行 |

关键命令与完整输出保留在本机 `/tmp/acomp-main-final.log`、`/tmp/acomp-core-final.log`、
`/tmp/acomp-windows-portable.log`、`/tmp/acomp-native-final.log`、
`/tmp/acomp-macos-clippy.log`、`/tmp/acomp-windows-clippy.log` 和两平台 build 日志。
额度专项 20 项、核心调度 12 项已包含在以上 Rust 结果中。
Swift 分组输出为 `/tmp/acomp-native-selected.log`、`/tmp/acomp-terminal-isolation-after.log`；
前台生命周期失败输出为 `/tmp/acomp-native-ui-final.log`，编辑器输出为 `/tmp/acomp-isolation-editor.log`。
继续执行时桌面已解锁，完整复测结果为 `/tmp/acomp-native-ui-unlocked.log`；此前三个
锁屏阻断的分组已全部通过，未放宽断言或跳过套件。

原生契约覆盖刷新写回位置、符号链接与硬链接、共享写入锁、分页历史、附件与工具结果、
分支依赖、压缩检查点、配置信任、真实终端恢复和正常退出。它们不验证真实账号计费或额度。
新增原生用例还验证同一 stdio worker 的实际认证配置、慢历史请求期间额度独立返回，
以及无效旧 profile 被严格拒绝而不回退默认配置。

对开工前记录的 5 个已安装程序/入口再次核对，解析目标与 SHA-256 均未变化；
本轮未执行 Codex/Dodex 安装迁移或官方更新。后续用户授权的 Companion 本机替换记录见下。

### 本地构建产物

- macOS 最终包：`dist/codex-dodex-20261002-macos-legacy-fix/agent-companion-v0.3.23-macos-arm64.zip`。
  已解包、验证严格 ad-hoc 签名，并实际运行包内 `agent-companion --version` 和
  `acomp --version`，两者均为 0.3.23。校验和位于同目录 `SHA256SUMS-macos-arm64.txt`。
- Windows 编译验证包：
  `dist/codex-dodex-20261002-windows-legacy-fix/agent-companion-v0.3.23-windows-x86_64-gnu-legacy-fix-validation.zip`。
  四个程序均为 x86-64 PE，ZIP CRC 和解包后逐文件哈希通过；包内明确标注 GNU debug
  验证构建，未执行 Windows 程序。校验和位于同目录 `SHA256SUMS-windows-validation.txt`。

Windows 验证包仅在工作区生成，未发布或安装；macOS Companion 包已按用户后续授权安装。

### 本机 Companion 替换

- 用户明确授权后，先完成解锁状态下的完整原生 UI 验收，再仅退出 Agent Companion。
- 将最终包同卷暂存，核验签名、完整文件树和 `--version` 后，通过 `renamex_np(RENAME_SWAP)`
  原子交换 `/Applications/Agent Companion.app`，随后用 LaunchServices 重新打开。
- 已验证最终兼容修复的安装内容与构建一致、签名有效，新进程 PID 为 81019。显示版本仍为 0.3.23；
  本次构建的主程序 SHA-256 为 `779b29c960254f8aa4d97cda4cf9ae82d60c2b9cea6d707e8608f2b2eab33a2e`。
- 最终修复的恢复记录：`/Applications/.AgentCompanion-install-20261002-w4bad9tn/record.json`；
  旧版应用保留在同目录 `swap.app`。需要回退时，先退出新 Companion，再用同目录
  `companion_swap.py rollback record.json`（传入记录的绝对路径）。
  初次替换前的更早版本仍保留在 `/Applications/.AgentCompanion-install-20261002-ucwtjyh6/swap.app`。
- 没有修改 Codex/Dodex 程序、命令入口或配置；受保护的 5 个程序/入口再次核验未变化，
  现存 Codex 进程保持运行。只替换 Companion 无需重启 Codex TUI。

### 本机安装后发现的旧 Dodex 兼容回归

初次替换 Companion 后，用户报告 Usage 面板只剩 Codex。只读检查确认 Dodex 的监控
仍然开启，安装和账号文件仍在；不是用户关闭监控或删除了副实例。

- 旧安装的配置显式指定文件认证，但允许 SQLite 和日志目录使用隔离启动器提供的路径
  及原生默认值。新的严格校验误把这两个缺省项当作冲突，导致已开启的 Dodex 从面板消失。
- 旧 Dodex 内置 CLI 为 `0.155.0-alpha.16.4`，不接受 `features.daemon_auto_start`。
  在严格配置模式下，该参数会使 Companion 的 stdio worker 启动失败。移除这个版本专属
  参数后，仍通过显式 stdio transport 和禁用 remote control 保持独立 worker。
- 回归修复保留真正冲突配置的拒绝行为，不改写个人配置；已开启但校验失败的实例保留
  可见错误卡片，同时不创建其账号查询或历史读取器。

旧版原生验证使用临时 HOME、合成凭据和本地 HTTP 服务，不读取真实账号。
修复先通过 Rust 路由回归、Swift Usage 模型回归和严格配置 worker fixture 复现失败，
再验证修复通过。定向验证包括核心配置 18 项、macOS 部署 77 项、路由 6 项、旧版 worker
1 项及 Swift 双实例额度套件；Rust 定向项已计入上方整体结果。
后续复测输出为 `/tmp/acomp-dodex-final-rust.log`、
`/tmp/acomp-dodex-ui-green.log`、`/tmp/acomp-dodex-native-0155.log` 和
`/tmp/acomp-dodex-native-0159.log`。

最终解锁后的完整原生 UI 测试通过，输出为
`/tmp/acomp-dodex-final-native-ui-unlocked.log`。macOS/Windows 全目标 Clippy、
两平台编译链接和 macOS 打包校验均再次通过；Windows 独立 `config-edit` 特性也通过
编译检查，系统配置目录使用与原生程序一致的 Known Folder API。

本机最终包的只读校验接受现存 Dodex 配置后完成原子替换。实际 Tasks 页恢复 Codex/Dodex
双卡，Usage 页可分别选择两实例，两侧额度均有成功读取时间；Dodex 手动刷新经历查询中
状态后更新成功时间，历史也正常显示。复查受保护的 5 个程序/入口不变、原有 Codex 进程
继续运行。本项仅验收此次卡片消失与旧 CLI 查询兼容回归，不代替换号、登出及成对升级矩阵。
公共记录不保存真实账号标识、额度明细或认证响应。

### 2026-10-03：Dodex 启动确认

用户将本机验收范围明确缩小为 App 和 TUI 能正常打开，不继续逐项功能矩阵。

- 通过已安装 Companion 的 `dodex-app --sync` 更新公共 Dodex App 启动器至打包修订 3。
  官方 App 和 Dodex App 均为 `26.928.21956`（构建 `12404`）；只同步 Dodex，保留原公共
  App 备份，未替换主 Codex TUI 的程序及资源。
- 实际启动 `/Applications/Dodex.app`，确认进入已登录主界面，无登录认证要求。
- 实际在 PTY 启动现有 `dodex --no-alt-screen`，确认显示工作目录、模型和交互输入提示；
  仅结束本次测试进程，Ctrl-D 正常退出，状态码为 0。没有发送模型任务或执行项目修改。
- 本机 Dodex TUI 仍由已有入口运行 `0.155.0-alpha.16.4`；本项验证正常启动，不声明
  独立 TUI 版本迁移或成对升级完成。

首次 Push 的 CI 同时发现两项问题：macOS 安装锁在并发 fork 继承文件描述符后可能短暂
残留，Windows 原生参数测试仍按没有隔离前缀的旧契约断言。安装锁已改为显式解锁，并以
继承描述符的确定性回归先复现再修复；Windows 测试校验隔离参数及原始参数、I/O 和退出码。
修复后 `acomp` 10 项测试、10 轮并行复测及两平台相关 Clippy 通过；Windows MSVC
运行结果以修复提交后的 GitHub CI 为准。

### 2026-10-03：`acomp` 配置兼容补丁与真实恢复启动

本节记录发布后当前补丁的结果，不修改已发布版本的验收历史。

- 修复显式 `enabled=false` 的 MCP 因残留相对命令或工作目录被误拦；原生最终配置仍须确认
  该服务停用。活跃相对 MCP、实际项目配置覆盖和其他原有安全检查保持限制。
- 项目扫描遵循 0.159.3 的默认 Git 根边界：目录型 `.git` 要求 `HEAD` 元数据可读，空目录
  继续向上查找，worktree 文件型标记仍有效。根以上另一账号配置不作为当前项目层。
- 已验证的 Dodex 部署可借用通过固定版本门槛的官方 App 原生程序；没有 App 原生候选时，
  仅检查主环境固定 standalone `current` 的完整包。历史、数据库位置、设置来源和明确选择
  的认证账号保持原路由，不执行程序更新或入口迁移。
- 接力核心定向测试 30 项通过；最终完整自动测试 **456 项通过**（核心 275、主程序 166、
  `acomp` 15），**2 项既有真实数据测试保持忽略**，CLI 集成 **6/6** 通过。
  macOS/Windows GNU 相关 Clippy、格式与差异检查通过。
  原生契约 **15 项分批通过**：外层沙箱中 13 项，另 2 项使用测试自身沙箱运行，避开
  macOS 嵌套沙箱限制。停用 MCP、真实 Git 边界、旧版历史往返和原项目可信度均有覆盖。
  旧版历史往返覆盖 `0.155.0-alpha.16.4 → 0.159.3 → 0.155.0-alpha.16.4`，使用临时目录、
  合成凭据和本地服务。
- 本机真实来源／账号四组合 **Codex/Codex、Dodex/Dodex、Codex/Dodex、Dodex/Codex**
  全部通过基本启动和退出。使用两个闲置原会话及其原项目目录，四次均通过 `config/read`、
  `account/read` 预检并进入原生 0.159.3 TUI；仅发送 Ctrl-D，退出码均为 0，终端属性、
  备用屏幕等状态恢复。当前原会话和账号标识不写入公共记录。
- 操作前备份两来源的 state/history 共四个数据库；真实库原已包含新版本字段。
  此次结果不证明旧 schema 迁移安全。没有发送模型 prompt，**不代表模型调用或订阅计费
  验收**；完整多轮对话、计费矩阵和 Windows 原生恢复仍未完成。

## 本轮改动

- **额度归属**：仅内存保存用户、工作区、认证存储、服务来源与请求代次；换号、登出或
  身份无法确认时清空额度和历史，拒绝晚到结果。正常 token 轮换保留同账号缓存。
  文件和系统凭据存储采用非交互读取，不输出凭据或身份指纹。
- **原生查询**：每个认证目录共用 Companion 自建 worker；额度与历史使用独立请求 ID、
  20 秒期限与结果发布。保留五分钟默认值、1–60 分钟设置、请求合并和失败标记。
- **双开隔离**：App、TUI、额度 worker 与更新进程共用环境清理规则；保留网络限制和必要
  代理/证书变量。验证实际认证、数据库、日志和 profile 配置；校验失败不能继续启动。
  修复 `--` 后提示文本、附着参数和选项值解析，镜像启动器修订提升至 3。
  macOS 面板生成的终端恢复命令也在实际执行时按同一规则清理，不将环境值写进命令文本。
- **入口迁移**：把版本、入口修订、App 绑定和更新路由一起纳入对齐判定；同版本也迁移
  已知旧模板，未知改动零覆盖。旧日常更新脚本转到统一维护，历史实现仅供显式恢复。
- **成对更新**：官方更新后重新读取实际版本并确认稳定渠道；出现不同版本或无法确认时
  不凭退出码报成功。macOS 统一锁顺序，检查完整替换目录里的进程，记录 App 切换恢复状态。
- **Windows**：精确 Store 产品 ID 的非交互 WinGet 入口、重新定位和验签；桌面与完整独立
  TUI 包分别记录，使用不可变版本目录和原子清单切换。非零“更新不适用”不当作已是最新版。

主要代码位于 `usage_service/`、核心 `usage_service.rs`、`process_environment.rs`、
`codex_args.rs`、`software_updates/` 和两平台部署模块；回归测试与对应模块一起提交审查。

### 已知支持边界

- 系统凭据库读取被锁定或需要交互授权时，额度显示不可用，不弹出授权窗口、不回用旧账号数据。
- 企业托管配置、加密 `secret_auth_storage` / `auth_keyring_backend`、进程内 ephemeral
  认证尚不能可靠确认身份，明确拒绝额度查询。不能把这一状态记为真实刷新验收通过。
- 原生 0.159.3 拒绝旧式根级 `profile` 选择器；额度查询不会继承另一个 TUI 的命名 profile。
  未选中的旧 `[profiles.*]` 表可以保留。实际有效配置仍须通过原生只读核验。
- 本地产物未发布；macOS Companion 已按授权本机安装，Windows 包未安装。
  macOS 包只作 ad-hoc 签名，未进行 Developer ID 公证。
  Windows GNU 交叉构建用于编译/链接验证，不能替代官方发布流程中的 MSVC 构建和 Windows 实机测试。

## 最终维护窗口

以下项目需要用户结束当前会话或在对应系统完成操作，本轮自动验证不能替代：

1. 保存现有 TUI 工作，并由用户退出 Codex / Dodex 的 App、TUI 和其残留辅助进程。
   从另一个终端或 Companion 发起维护，避免替换正在执行维护的入口。
2. Companion 本机替换已完成；在退出受影响程序后运行统一对齐/更新流程。遇到占用、签名、权限或
   未知入口修改错误时，按具体错误处理；不要手工跳过验证或删除恢复记录。
3. macOS 如出现系统授权，由用户完成。Windows 需要安装/启用 WinGet，确认组织策略和
   Microsoft Store 协议允许更新；权限或策略阻断须记录实际错误。
4. 分别打开四个入口：Codex App、Dodex App、Codex TUI、Dodex TUI。
   检查版本按两组成对一致，并验证 `dodex app [项目路径]` 与 `dodex update` 的目标。
5. 分别确认两个账号登录；同一实例的 App/TUI 应关联同一账号与历史，两实例间独立。
   关闭 Companion 监控后再次确认启动和历史发现。
6. 同时读取两个真实账号额度，记录读取时间和成功/失败状态（不记录凭据、身份指纹或
   完整认证响应）。测试一侧失败、换号、登出、恢复登录，确认不会显示另一账号旧额度。
7. 验证五分钟默认刷新、1–60 分钟配置、手动刷新、休眠恢复和慢历史查询时额度独立完成。
8. Windows 实机重复双开、项目打开、TUI 参数/退出码/信号、WinGet 更新、重新定位与验签、
   独立 TUI 包更新和失败后重试；填写实际版本、系统版本和结果。

只有版本、入口路由、账号归属和真实刷新全部通过，才标记对应平台功能验收完成。
目前两实例正常额度读取和 Dodex 手动刷新已观察通过；完整换号/登出矩阵、现存安装迁移
和 Windows 实机验收仍未完成。

### 实机记录（维护窗口填写）

| 项目 | macOS | Windows |
| --- | --- | --- |
| 系统版本 / Companion 构建 | macOS 26.5.1；Companion 0.3.23 本次修复构建已安装 | 待填写 |
| Codex App / Dodex App 实际版本 | 均为 26.928.21956 / 12404；Dodex 打包修订 3 | 待更新后核验 |
| Codex TUI / Dodex TUI 实际版本 | Dodex 旧入口 0.155.0-alpha.16.4 启动通过；成对升级未验收 | 待更新后核验 |
| 四入口、项目打开、账号与历史关联 | Dodex App/TUI 正常启动通过；其余矩阵未验收 | 未验收 |
| 两账号真实额度、换号与登出 | 正常读取及 Dodex 手动刷新通过；换号与登出未验收 | 未验收 |
| `acomp` 原会话来源／账号四组合 | 当前补丁构建 4/4 基本启动、预检及退出通过；未发模型 prompt、未验计费 | 恢复启动仍禁用，未实机验收 |
| 成对升级、二次运行、失败恢复 | 未验收 | 未验收 |
| 授权 / Store 策略阻断及错误 | 待填写 | 待填写 |

统一维护可从 Companion 的双开设置启动；迁移后的 `dodex update` 进入同一流程。
需要自动化维护时，使用完整 Companion 应用可执行文件的
`software-maintenance align` 或 `software-maintenance update-all`，不要把仅提供终端功能的
`acomp` 或指向它的别名当作完整应用维护入口。

## 官方接口依据

- [认证存储与自动刷新](https://learn.chatgpt.com/docs/auth)
- [Windows 官方产品 ID 与部署方式](https://learn.chatgpt.com/docs/enterprise/windows-deployment)
- [WinGet upgrade 精确匹配与非交互参数](https://learn.microsoft.com/en-us/windows/package-manager/winget/upgrade)
