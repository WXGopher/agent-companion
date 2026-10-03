# `acomp resume`

`acomp resume` 的目标是继续写入原会话，只选择本次使用 Codex 或 Dodex 的额度。会话 ID、历史存储位置和本次额度账号是三件不同的事。切换账号不应创建另一份会话，也不应靠摘要重建上下文。

## 命令

在项目目录运行：

```sh
acomp resume
acomp resume --list
acomp resume SESSION_ID --details
acomp resume SESSION_ID --source codex --account dodex
acomp resume SESSION_ID --account codex --profile work --details
agent-companion resume SESSION_ID --account dodex
acomp --help
acomp --version
```

不指定 ID 时打开会话选择器，输入文字搜索标题、ID 或来源，方向键选择，Enter 继续，Tab 展开详情，PgUp / PgDn 滚动详情，Esc 返回。下一步选择本次额度账号；不兼容账号显示原因并禁用。相同 ID 在两个历史环境都出现时，显式指定 `--source`，不会自动挑选最近一份。

`--list` 和 `--details` 不启动恢复。`--profile` 指定历史来源环境的命名配置；它不改成使用额度账号的配置。没有自动额度切换。

运行 `agent-companion install-cli` 安装用户级入口，无需管理员权限；安装器不会覆盖无关的同名命令。发布包内也包含 `acomp` / `acomp.exe`。安装后的可用目录与 PATH 操作以安装命令输出为准。

Windows 安装到用户 PATH 的两个入口均为控制台程序，提供 `resume`、`install-cli`、帮助和版本。发布包中的 `agent-companion.exe` 保留完整 GUI 与原有子命令。

## 当前兼容性

当前实现是保守的首版适配器，尚未完成计划中的全部实机验收：

- macOS 的原生恢复适配固定为 Codex CLI **0.159.3**，要求文件式 ChatGPT 登录和来源配置明确指定模型。其他版本不能通过版本门槛。
- 已保存的 Dodex 部署和托管入口先通过原有校验。其运行程序不兼容时，可借用固定官方 App 位置发现的合格原生程序；App 不含该程序时，再检查主环境的固定 `packages/standalone/current/bin/codex`。standalone 候选必须具有完整安装包布局，所有候选均须通过同一 0.159.3 版本检查。借用仅改变本次运行程序，历史、数据库位置、个人配置仍来自所选历史环境，认证仍来自明确选择的账号；不执行升级或改写已安装入口。
- Windows 已实现命令、安装、历史发现、目录 junction 和同卷文件硬链接；**恢复启动仍禁用**，直到 Windows 原生写回、终端和完整恢复验收完成。交叉编译通过不解除该限制。
- 命名 profile、项目 `.codex/config.toml` 覆盖、无法保留语义的相对配置路径、托管策略、其他 provider 和自定义 OpenAI 服务地址当前禁用。`--details` 说明具体原因；不会删除这些配置或降低安全策略来启动。
- 显式设置 `mcp_servers.<name>.enabled = false` 的服务允许保留相对 `command`、`cwd` 和失效的旧工具路径，普通表与内联表均支持。原生预检必须确认这些服务在最终有效配置中仍为 `enabled=false`。启用或省略 `enabled` 的 MCP 仍接受原有路径和认证检查；此修复未开放活跃 MCP 相对路径，也不自动停用工具来绕过检查。
- 无法保留本地工具 OAuth 存储语义的 MCP 配置同样禁用，包括相关钥匙串存储和不能安全写回的本地凭据文件；不会将“配置文件已链接”当成工具认证已保持的证明。
- 本次模型由来源环境的当前有效设置决定，启动时明确传入，避免原生恢复另行采用旧会话的模型。未能确认的配置不显示成已验证可用。
- 原生 `account/read` 用来核对进程实际加载的 ChatGPT 工作区账号；本地模拟服务测试不能证明真实服务端订阅扣费，未完成真实四组合计费验收。

## 来源与兼容性

会话列表只显示当前工作目录精确匹配的会话。历史发现独立于 Companion 的监控开关。选择会话后，来源信息区分：

| 项目 | 含义 |
| --- | --- |
| 历史存储来源 | 实际存放原会话的 Codex / Dodex 环境和会话 ID |
| 本次额度 | 本次连接的文件式 ChatGPT 认证账号 |
| 本次设置来源 | 历史所在环境当前可读取的个人指令、记忆、模型和工具配置 |
| 原生运行程序 | 本次实际使用且通过兼容检查的二进制；可能借用主环境的合格程序 |
| 项目指令 | 当前项目的指令文件及项目配置 |
| 原始创建来源 | 没有可靠创建来源记录时显示未知，不从最近使用的账号推断 |

来源说明针对本次可检查的本地文件，不承诺恢复已删除或修改的历史配置，也不声称账号服务端的个性化行为来自原账号。详情不得输出凭据、密钥或完整配置内容。

接力前必须验证运行程序、认证存储、配置及会话占用。钥匙串认证、不能保持相对路径语义的配置、其他 API provider、未知认证写入行为及其他未验证组合均须显示原因并禁用。不得为通过检查自动改写原账号的登录方式或安全策略。

项目配置与指令扫描遵循固定原生版本的默认 Git 边界：从当前目录向上找到最近的有效 `.git`，目录型标记要求 `HEAD` 元数据可读，文件型 worktree 标记也有效；没有有效标记时仅检查当前目录。根目录以上另一账号的 `.codex/config.toml` 不属于当前项目。边界内实际存在的项目配置覆盖仍按现有兼容限制禁用，自定义 `project_root_markers` 也未开放。

## 存储约定

受管理的运行目录连接原环境的历史、附件、记忆和配置资源，数据库明确使用原目录。整个 `thread-writer-locks` 目录共享，包括协调锁，确保原生客户端和接力使用同一写入锁命名空间。

认证连接所选账号的 `auth.json`。macOS 使用符号链接；Windows 的文件认证设计使用同卷硬链接。仅可在原生写回行为已验证的版本启用，不能假定所有版本都会原地更新文件。启动前重新确认账号及链接身份；重新登录、文件替换或版本升级后必须重新检查。

原生进程按明确的 session ID 恢复，禁止共享 daemon。找不到该 ID 时退出，不使用 `--last`，不新建会话。正常退出与异常恢复只清理本工具管理的资源，不能删除链接指向的历史和认证文件；仍有活跃子进程时不能回收其运行目录。

清单在启动子进程前先持久化“启动中”状态。如果父进程在启动后、记录 PID 前崩溃，无法确认该进程是否仍活跃，后续恢复会保留这份运行目录。它不会为了清理彻底而猜测进程已退出。账号选择也绑定当时展示的工作区和用户身份；选择后重新登录不能悄悄切换成另一用户。

## 验收边界

单元测试、模拟运行程序或交叉编译不能证明真实订阅计费正确，也不能代替 Windows 实机 TUI 验收。开放某个组合前，需要分别验证：

- Codex → Codex、Codex → Dodex、Dodex → Codex、Dodex → Dodex，往返后仍是同一 ID 和连续历史。
- 多轮对话、工具结果、压缩历史、分页历史、附件及分叉依赖经原生恢复保留。
- 实际认证账号、刷新写回位置、指令、模型、工具和配置层级与来源详情一致。
- 原生客户端占用、两次并发启动、重新登录、终端退出、崩溃、配置修改与版本升级。
- Windows 无管理员权限安装及链接；macOS / Windows 的真实终端选择、恢复和退出。

运行目录设计参考 [codex-auto](https://github.com/xhyqaq/codex-auto)。移植范围、上游版本与许可证见 [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。

## 初始验证记录（2026-10-01 至 10-02，macOS arm64）

- 核心完整测试 **253 项通过、2 项原有忽略项**，其中新增 22 项接力回归：原历史共享、明确 ID、原生锁命名空间、身份固定及重新登录、路径与配置拒绝、原生默认服务地址、账号刷新写回和崩溃清理边界。
- 独立 CLI 单元测试 6 项、CLI 集成测试 5 项通过，覆盖两个入口、当前目录精确匹配、关闭监控后的 Dodex 发现、无交互输入时禁止自动选择，以及错误信息不泄露配置内容。
- 实际 `acomp` PTY 覆盖搜索、来源／额度页面、展开详情、禁用账号、Esc 取消和终端恢复。测试会持续消费 PTY 输出，避免测试端输出积压阻塞退出。
- Codex 0.159.3 的 **9 项原生契约测试通过**：多轮同 ID 往返、分页、工具结果、图片附件、分叉依赖、压缩检查点、认证链接原地刷新、原生写入锁、配置覆盖，以及原生 TUI 恢复／当前模型／所选账号／Ctrl-D 退出。测试使用临时目录、伪造凭据和本地服务，不读取真实账号或产生真实订阅消费。
- 另有 **1 项实际 `acomp` 入口的预检失败测试通过**，与上述 9 项一起运行共 10 项通过。普通来源配置通过检查并到达原生 `account/read`；macOS 沙箱限制子进程只能连接本机，代理拒绝外部连接。账号验证失败后清理接力目录，保留原历史、分页数据库和两份认证文件，未新增模型请求。此测试发现并修复了把原生自动补出的默认服务地址误判为自定义覆盖的问题；显式覆盖仍禁用。
- macOS 全目标 Clippy、Windows GNU workspace 全目标 Clippy（均 `-D warnings`）、格式和差异检查通过。macOS 打包及解包后的两个入口版本检查通过，包内包含入口、说明和许可证。Windows 实机运行与 ConPTY、真实账号四组合扣费、完整 `acomp prepare → 原生 TUI` 联合验收仍未完成。
- 完整应用回归中的 `startup_publishes_limits_while_history_is_still_waiting` 在本机存在基线超时：改动前的缓存测试程序同样失败，实测夹具启动约 1.03 秒，超过该测试 800 毫秒期限。未调整生产代码或放宽期限；过滤这一个已确认的基线测试后，其余应用测试及原有 macOS 编辑器实机测试通过。

## 配置兼容修复验证（2026-10-03，macOS arm64）

- 两条实际模式先由回归复现：显式停用的 MCP 相对 `command` / `cwd` 被误拦，Git 根以上另一账号配置被误算为项目覆盖。另补空 `.git` 不能遮蔽有效上层 Git 根的回归。修复后接力核心定向测试 **30 项通过**；最终完整自动测试共 **456 项通过**（核心 275、主程序 166、`acomp` 15），**2 项既有真实数据测试保持忽略**，CLI 集成 **6/6** 通过。macOS 与 Windows GNU 的相关 Clippy、格式及差异检查通过。
- 原生契约最终 **15 项分批通过**：13 项使用外层沙箱，另 2 项使用测试自身沙箱，避开 macOS 不允许嵌套沙箱的限制。其中新增停用 MCP、Git 边界、旧版历史往返契约与既有项目可信度契约的定向验证 **4/4** 通过。临时来源目录与链接后的运行目录返回相同停用 MCP 配置；原生 `thread/start` 与本地模拟 turn 均未启动其残留工具。有效 Git 根内的配置仍由原生识别，上层另一账号配置被排除。
- 旧内置原生程序 **0.155.0-alpha.16.4 → 0.159.3 → 0.155.0-alpha.16.4** 的临时历史往返通过，覆盖原会话、分页历史及共享写入锁。该结果限定于合成夹具，不代表任意旧数据库迁移或真实账号计费验收。
- 另在本机两个闲置原会话及其原项目目录验收真实来源／账号四组合：**Codex/Codex、Dodex/Dodex、Codex/Dodex、Dodex/Codex，4/4 通过**。当前补丁构建均通过原生 `config/read`、`account/read` 预检并进入 0.159.3 TUI；仅发送 Ctrl-D，均以状态码 0 退出，终端属性和备用屏幕恢复。未发送模型 prompt，因此不构成模型调用或订阅计费验收。
- 实机操作前备份了两来源的 state/history 共四个数据库。本机数据库原已包含新版本字段，此次启动结果不能替代旧 schema 迁移验收。合成原生契约使用临时目录、合成凭据和本地服务；Windows 原生恢复、活跃 MCP 相对路径以及真实四组合多轮对话／计费仍未验收。

复现原生测试：

```sh
ACOMP_TEST_CODEX_BINARY=/absolute/path/to/native/codex \
  ACOMP_TEST_LEGACY_CODEX_BINARY=/absolute/path/to/legacy/native/codex \
  ACOMP_TEST_ACOMP_BINARY=/absolute/path/to/acomp \
  python3 -m unittest discover -s scripts/tests -p test_resume_native.py -v
python3 scripts/test-resume-tui.py --binary target/debug/acomp
cargo test -p agent-companion-core --features resume,server --locked
cargo test -p agent-companion --test resume_command --locked
```

未设置 `ACOMP_TEST_CODEX_BINARY` 时，原生测试明确跳过；旧版历史往返另需 `ACOMP_TEST_LEGACY_CODEX_BINARY`，实际 `acomp` 预检失败测试另需 macOS 和 `ACOMP_TEST_ACOMP_BINARY`。测试拒绝 shell 包装器和不匹配的版本，避免包装器重写 `CODEX_HOME` 而进入真实账号环境。
