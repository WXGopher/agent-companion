# Codex / Dodex 修复与验收记录

本轮验收日期：2026-10-02。验收分为 `Codex App = Dodex App` 与
`Codex TUI = Dodex TUI` 两组；不要求 App 与 TUI 使用同一版本号。

## 执行边界

- 不退出、重启或替换现存 Codex TUI 及其资源，不修改其登录状态。
- 自动测试只使用临时目录、合成凭据、测试进程和本地服务。
- 不连接或清理用户现有 daemon，不运行实际账号额度查询。
- 本地构建、交叉编译和打包不等于部署完成；Windows 交叉编译不等于 Windows 实机验收。
- 真实账号读取可能触发原生客户端自动刷新凭据，因此集中到最终维护窗口。
  参见[官方认证说明](https://learn.chatgpt.com/docs/auth)。

## 验收证据

以下为本轮实际执行结果。不同测试目标会重复导入部分模块，不把各行相加作为独立用例总数。

| 检查 | 实际结果 |
| --- | --- |
| 核心 `--all-features` | 264 项通过，2 项原有忽略 |
| 主程序 `--all-features` | 155 项通过 |
| `acomp` 单元测试 | 9 项通过 |
| 便携 `windows_settings` 测试目标（在 macOS 执行） | 146 项通过；含导入的共有模块测试 |
| 其他集成测试 | resume_command 5、windows_indicators 9、windows_updates 1、windows_usage 25 项通过 |
| Python manager 测试 | 24 项通过 |
| Python scripts/tests | 55 项通过，12 项原生测试默认跳过后单独执行 |
| 原生 Codex 0.159.3 + 最新调试 `acomp` 合成账号契约 | 12 项全部通过，无原生项遗留跳过 |
| 托管 Dodex wrapper 的原生 session/resume | 通过 |
| `acomp` PTY 交互与退出 | 通过 |
| Swift 终端恢复命令环境隔离 | 先复现失败再修复通过；真实 shell 子进程验证 14 类覆盖变量清理及沙箱/网络/代理保留 |
| Slint/AppKit 真实编辑器 | 34 个阶段通过；测试显式请求重绘以覆盖锁屏后的后台帧调度 |
| Swift 原生 UI 解锁后完整复测 | 10 个原生套件全部通过；Rust 验收目标 1 passed / 0 failed，耗时 137.32 秒 |
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

- macOS 最终包：`dist/codex-dodex-20261002-macos-final/agent-companion-v0.3.23-macos-arm64.zip`。
  已解包、验证严格 ad-hoc 签名，并实际运行包内 `agent-companion --version` 和
  `acomp --version`，两者均为 0.3.23。校验和位于同目录 `SHA256SUMS-macos-arm64.txt`。
- Windows 编译验证包：
  `dist/codex-dodex-20261002-windows-validation/agent-companion-v0.3.23-windows-x86_64-gnu-validation.zip`。
  四个程序均为 x86-64 PE，ZIP CRC 和解包后逐文件哈希通过；包内明确标注 GNU debug
  验证构建，未执行 Windows 程序。校验和位于同目录 `SHA256SUMS-windows-validation.txt`。

Windows 验证包仅在工作区生成，未发布或安装；macOS Companion 包已按用户后续授权安装。

### 本机 Companion 替换

- 用户明确授权后，先完成解锁状态下的完整原生 UI 验收，再仅退出 Agent Companion。
- 将最终包同卷暂存，核验签名、完整文件树和 `--version` 后，通过 `renamex_np(RENAME_SWAP)`
  原子交换 `/Applications/Agent Companion.app`，随后用 LaunchServices 重新打开。
- 已验证安装内容与最终构建一致、签名有效，新进程 PID 为 65747。显示版本仍为 0.3.23；
  本次构建的主程序 SHA-256 为 `34e72acabdb3ba12a2c1419d98215447b45c41b366cade8f22c053a5e82dc62a`。
- 恢复记录：`/Applications/.AgentCompanion-install-20261002-ucwtjyh6/record.json`；
  旧版应用保留在同目录 `swap.app`。需要回退时，先退出新 Companion，再用同目录
  `companion_swap.py rollback record.json`（传入记录的绝对路径）。
- 没有修改 Codex/Dodex 程序、命令入口或配置；受保护的 5 个程序/入口再次核验未变化，
  现存 Codex 进程保持运行。只替换 Companion 无需重启 Codex TUI。

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
目前真实双账号验收、现存安装迁移和 Windows 实机验收均未完成。

### 实机记录（维护窗口填写）

| 项目 | macOS | Windows |
| --- | --- | --- |
| 系统版本 / Companion 构建 | macOS 26.5.1；Companion 0.3.23 本次修复构建已安装 | 待填写 |
| Codex App / Dodex App 实际版本 | 待更新后核验 | 待更新后核验 |
| Codex TUI / Dodex TUI 实际版本 | 待更新后核验 | 待更新后核验 |
| 四入口、项目打开、账号与历史关联 | 未验收 | 未验收 |
| 两账号真实额度、换号与登出 | 未验收 | 未验收 |
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
