# v0.2.0 验证记录

## 本地验证（2026-09-15，Apple Silicon）

使用 Rust 1.98.1、Codex CLI 0.154.0；已安装并登录 GitHub CLI 2.97.0。

| 项目 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| macOS 主程序与公共核心测试 | 2 个编辑器草稿测试、158 个核心测试通过；1 个读取真实会话的测试显式忽略 |
| `cargo test -p agent-companion-core --features server --locked` | 172 项通过；2 个显式忽略项 |
| macOS Clippy，全部 targets，`-D warnings` | 通过 |
| Windows GNU x86_64 交叉编译和 Clippy，整个 workspace/全部 targets | 通过；不等同于 Windows 运行测试 |
| macOS release 构建与 `.app` 打包 | 通过；arm64，版本 0.2.0，仅依赖系统动态库，ZIP 校验和验证通过 |
| macOS 依赖范围 | 编辑器构建不含后端 server feature、SQLite、托盘或 WebSocket relay |

覆盖配置缺失、空列表、恢复默认、未知组件、原顺序、多行注释、BOM、inline TUI、无关配置的并发修改、状态栏冲突、只读/写入失败、符号链接和权限保持；覆盖新旧环境变量优先级、配置复制迁移、旧 hooks 去重升级及原始卸载记录恢复。

## macOS 实机

在隔离的 `CODEX_HOME` 中操作真实 Slint 窗口：

- 命令默认入口与 `codex-tui` 均能启动独立编辑器。
- Space 切换、Tab 移动焦点、实时预览正常；Apply 前文件未变。
- Apply 保存后保留未知项、顺序、注释及主题，并生成备份；重新打开读回相同选择。
- Restore 仅移除 `tui.status_line`；全部取消后 Apply 保存 `[]`。
- 外部修改状态栏后 Apply 显示冲突错误，外部文件内容保留。
- 关闭窗口后退出码为 0，无编辑器残留进程。
- 编辑器退出后启动 Codex CLI 0.154.0，实际 footer 显示保存的模型组件和 Git 分支；未知 fixture 项被 Codex 自己忽略。没有提交模型任务。

## 远端验证（2026-09-16，UTC+8）

- 已将原仓库直接改名为 `WXGopher/agent-companion`，页面确认原有 1 个 Star 保留。本地 origin 已更新。
- [CI run 34994024326](https://github.com/WXGopher/agent-companion/actions/runs/34994024326) 在源码 `08f1105` 上通过 Windows MSVC x86_64 和 macOS arm64 的格式、构建、Clippy、测试及隐私扫描。
- Windows 共通过 331 项测试：主程序 137、启动器 1、端到端 19、核心 172、hook 2；另有 10 项依赖真实桌面、网络或本机会话的测试显式忽略。公共核心测试另以 `server` feature 再次通过。
- macOS 通过 2 项编辑器测试、158 项核心测试，以及启用 `server` feature 的 172 项公共测试；`.app` 打包通过。
- Windows 按本轮交付边界以编译通过为验收，未进行人工桌面回归。管道及桌面集成测试仅在 Windows 编译/执行；显式忽略的真实桌面测试可按 README 运行。
- [Release run 34994066105](https://github.com/WXGopher/agent-companion/actions/runs/34994066105) 全部通过，已发布 [v0.2.0](https://github.com/WXGopher/agent-companion/releases/tag/v0.2.0)。两个 ZIP 都从该标签的 `08f1105` 源码构建。
- 已下载正式发布的两个 ZIP，确认压缩包完整性、Windows 三个程序与 macOS `.app` 的文件布局，并通过 `shasum -a 256 -c SHA256SUMS.txt`。汇总校验文件已统一为 LF 换行，后续发布流程也会自动归一化 Windows 换行符。
- 两个 ZIP 均通过 `gh attestation verify`，限定本仓库、`.github/workflows/release.yml`、`refs/tags/v0.2.0`、完整源码提交及 GitHub 托管 runner。
- 下载的 macOS 程序报告版本 0.2.0，arm64 架构、应用元数据和资源正确，仅依赖系统动态库。最后一次下载包窗口复检时所有应用窗口均无法访问，因此未计为成功；测试进程已清理。上方本地构建的 macOS 实机结果仍为实际验证结果。

## 发布约定

GitHub Actions 从 `v0.2.0` 标签构建两个 ZIP，汇总 `SHA256SUMS.txt` 并为每个平台的 ZIP 生成来源证明。macOS 首版无 Developer ID 签名或 Apple 公证；Rust 链接器的 ad-hoc 签名不是 Developer ID 签名。首次打开方式见 README。
