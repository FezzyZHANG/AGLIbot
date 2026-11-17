# Hands、任务、进化与运维审查

日期：2026-11-15。只读审查当前 checkout；仅在 `target/audit-20261115/` 写审查证据。未改 `src/`、`config/`、运行时配置、Moldloft 数据或任何真实进程；未读取 `.env` 值、调用付费模型、执行真实部署/进化。完整 Cargo 检查由主审查进程执行，本分工不重复运行。

## 确定发现

### HEO-01 — [P1] 进化候选可以基于过期版本发布，绕过手动部署的防回滚保护

- 主位置：`src/evolution/gate.rs:313–315`、`344`、`469–476`、`575–578`。
- 完整触发链：当前运行 release 已是 B（或第一次进化 E1），第二个提案仍声明旧 A 为 `base_commit`。`check()` 只确认 A 是一个完整 commit，然后从 A 创建 detached worktree；审查 diff 也是相对 A。`finish_inner()` 复核的仍是候选 worktree 的 A/tree 和 Moldloft 来源，不读取当前 release。`commit-tree` 的唯一 parent 为 A；release 按 A+patch 构建并交给 restart。因此新版本会丢失 B/E1 的全部其他变化，而这些回滚没有出现在本次审查 diff 中。
- 影响：已经上线的修复、包括旧基线之后的安全闸门修复，可能被一个仅声明普通 `src/` 修改的提案隐式撤销。审批看到的是相对旧 A 的小补丁，并未展示相对在线版本的完整行为变化。
- 为什么不是既有明确限制：`ops/deploy.ps1:25–31` 已使用 `merge-base --is-ancestor` 拒绝相同情况，`docs/DEPLOY.md:34–35` 和 `docs/EVOLUTION.md:103–105` 把防止丢失已启用进化作为部署契约；文档的 worktree 不提供 OS 隔离，与版本关系检查无关。
- 修复建议：候选准备时固定当前 release 的身份与 commit；确保该 commit 是候选的祖先（普通 v1 可直接要求 base 与当前 release 相同）；提交/交接前再次确认 release 未漂移。把相对当前上线版本的 diff 和 release 基线写入审查证据；不同基线要求新提案，不能用普通审批覆盖隐含回滚。
- 验收：合成 Git 仓库 A→B，运行指针 B、候选基于 A；应在构建前拒绝。再覆盖“prepare 时为 A，审批前变成 B”以及两次未合并进化，确保第二次不能丢失第一次。现有 `changed_candidate_or_moldloft_source_invalidates_approval` 仅改候选工作文件/工单版本，不覆盖运行 release。
- 验证级别：静态完整触发链；没有实际发布。

### HEO-02 — [P2] 进化后的新进程没有更新启动脚本的 PID 记录，后续正常重启失败

- 主位置：`src/evolution/restart.rs:156–166`；关联 `ops/start-bot.ps1:12–27`、`ops/deploy.ps1:64–68`。
- 完整触发链：手动启动写 `data/logs/current.json` 记录 PID P。进化 helper 等 P 退出后启动新 PID Q，但既不写 Q，也没有其他代码更新该记录。之后 `start-bot -Restart` 只尝试停止 P；P 已退出，Q 保留；全局 aglibot 查询发现 Q，抛出 `AGLI is already running`。通过 `deploy.ps1` 调用时，release.json 已先被写成新目标，但在线 Q 未切换，进一步造成指针与实际进程不一致。
- 为什么不是既有明确限制：这是正常成功进化→后续手动部署的路径，不是文档注明的崩溃后自动重放/自动回滚限制；文档把这两种发布路线描述成共同使用一个 release 指针。
- 修复建议：进化成功启动时写入与手动启动兼容的当前实例记录（PID、exe、root、进程创建标识、日志、commit）；启动脚本按同一实例身份验证与接管。发布指针的变更应与已验证启动结果协调，失败保留/恢复旧指针或明确记录 pending。
- 验收：合成进程表里 current.json=P、P 已退出、Q 运行，`-Restart` 必须识别并接管 Q；额外覆盖 PID 复用，不能误杀其他实例。模拟下一次 deploy 启动失败时指针不能谎报新版本已运行。
- 验证：安全夹具 `ops-mock-check.ps1` 执行真实 `start-bot.ps1`，所有进程/散列 API 为模拟，已复现拒绝接管 Q；没有启动或停止真实进程。

### HEO-03 — [P2] 工作流水线忽略审查结论，把暂缓/待修改/待主人决定的任务标为 Done

- 主位置：`src/work/research.rs:750–757`、`789–796`；关联 `48–50`、`src/agli.rs:1994–2000`、`src/mind/judge.rs:121`。
- 完整触发链：研究流程在有限修订后仍得到 `revise`，或任何一次得到 `defer` / `owner`；编码流程得到任何非 `pass`。两条路径都返回 `ok:true`、无 clarification/paused，`Outcome::status()` 于是返回 Done。`on_job_done()` 持久化 Done 并写“完成”日记。研究只在总结里附 caveat；编码总结甚至只附执行者说明和单独的 verdict 字符串。`judge::file()` 对 revise/defer 默认 Closed，故这两类失败还会从未决汇总消失；Owner 虽有开放审查，但任务本身已关闭。
- 影响：看板与人格层均认为任务完成；`/run` 不再接受 Done 任务，模型审查“暂缓/修改后再审”的决策失去实际效力。
- 为什么不是既有明确限制：有限修订可合理停止，但不能把“停止尝试”和“通过验收”混为一个完成状态；DESIGN 5d 明确区分四种审查结果，进化流水线也确实以 verdict 控制可批准状态。
- 修复建议：Outcome 保留结构化 judgement 与完成条件。仅 Pass 且没有待处理要求时 Done；其他结果 Waiting，并保留所需审查/修改意见和手动继续入口。将 revise/defer 的队列归档行为与任务是否可执行明确对齐。
- 验收：stub reviewer 分别返回四种 verdict，覆盖研究修订耗尽和 code 结果；非 Pass 应留 Waiting、可再次 `/run`，待主人项进入 digest，日记不能写完成。
- 验证级别：静态完整触发链。已有 tests 覆盖修订次数、clarification、即时反思和 followups，却未覆盖最终 verdict→看板状态的矩阵。

### HEO-04 — [P2] `-Restart` 先停止健康实例，再校验下一版本；候选损坏会直接造成停机

- 主位置：`ops/start-bot.ps1:12–24` 与 `29–31`；文档 `docs/DEPLOY.md:39` 声称“先校验指针里 exe 的 sha256”。
- 触发：当前 bot 正常运行，release 指针指定的下一 exe 丢失、不可读或 SHA-256 不符。脚本已经 Stop-Process / WaitForExit 并关闭窗口，然后才 Get-FileHash；校验异常终止脚本，原先可用的 bot 已停。
- 为什么不是既有明确限制：不要求自动回滚，也不需要真实启动才能提前确认文件和哈希。这是预检顺序错误，并与文档明文相反。
- 修复建议：在任何 Stop-Process 前读取/校验候选指针、exe、root/source 和必要日志目录；保留校验过的候选身份，切换前防止被替换。准备成功后才停止目标实例。
- 验收：真实脚本配模拟 Stop-Process 与散列失败，断言散列失败时 Stop-Process 调用数为 0。还应覆盖缺少日志目录的首次部署：当前两个 start 脚本均未创建 `data/logs`，cmd 的 stderr 重定向会先于 bot 初始化失败。
- 验证：`ops-mock-check.ps1` 已成功复现健康模拟 PID 在散列失败前被停止；未操作真实进程。

### HEO-05 — [P2] 编码执行期间追加的主人补充被静默遗漏，任务仍关闭

- 主位置：`src/work/research.rs:768–774`、`780–796`。
- 完整触发链：编码任务开始时读取 followups A，委派执行期间主人通过聊天追加 B；Board 确实把 B 记录在 followups.md/log.md。delegate 返回后，`code()` 只使用旧的 instructions A 来审查，不再读取文件，也没有比较未处理后缀。随后无条件成功完成。任务 Done 后 `Board::follow_up()` 拒绝后续补充，`/run` 也不再接受，主人以为已合入的 B 实际未进入执行/审查/结果提示。
- 为什么不是既有明确限制：DESIGN 10b 的步骤边界机制确实主要描述研究流水线，因此不要求编码中途打断 CLI；但最基本的完成前复读和明确提示即可避免已接受的补充静默丢失。研究路径在 `739–745` 已实现这一提示，code 路径缺失。
- 修复建议：delegate 返回与审查完成处重新读取一致快照，与已消费位置比较；有新要求则作一次有界续跑，或保留 Waiting 并明确列出未处理内容，不能无声 Done。持久化消费位置可另作后续增强。
- 验收：用无模型 fixture 阻塞 delegate，在其返回前追加 followup；断言新内容要么进入续跑/审查，要么结果明确写“未处理”且状态可继续。覆盖审查期间追加和最后边界追加。
- 验证级别：静态完整触发链；无真实 Codex 调用。

### HEO-06 — [P2] 支持自定义 data_dir，但运维脚本始终读写 `<runtime>/data`

- 主位置：`ops/start-bot.ps1:10,29,34–35`、`ops/deploy.ps1:24,53`、`ops/start-stickers-web.ps1:10,21–23`；运行端 `src/config.rs:482–483`、`src/evolution/gate.rs:202,543`。
- 触发：配置合法的 `data_dir: state` 或绝对路径。bot 在自定义目录读取数据并更新 release，而 start/deploy/sticker 脚本继续使用固定 data/release.json / data/logs。进化更新的提交对手动部署防回滚检查不可见，启动脚本可继续读取旧指针，启动日志也会显示 no release pointer 或 mismatch。
- 为什么不是既有明确限制：`data_dir` 是已支持的配置，EVOLUTION 明确大部分路径跟随它；DEPLOY 没有声明运维模式禁止自定义数据根。这不是 OS 隔离或自动恢复延期。
- 修复建议：给脚本统一提供/解析规范 data root（可保存在 runtime.json，避免 PowerShell 自行重复一套 YAML 解析），或在部署入口显式拒绝当前不支持的数据目录配置；文档必须说明唯一的指针解析规则。
- 验收：默认、相对自定义、绝对 data_dir 三种 runtime 下，bot/进化/手动部署/页面启动必须读取同一个 release 和实例记录。尤其覆盖自定义目录已有 evolution commit，手动回滚应被拒绝。
- 验证级别：静态配置与消费者路径核对。

## 设计、文档和实现对齐矩阵

| 契约/说明 | 当前证据 | 对齐动作 |
|---|---|---|
| README:5,41–43 说研究和编码都交 Codex；DSH:3 说 research defaults to Codex | 类型缺键默认确实 Codex，但仓库 runtime.yaml:32 显式设 dsh；research.rs:501 按配置分派 | 区分“缺键默认”“仓库出厂配置”“当前在线配置”，README 用当前配置说明研究走 DSH、编码走 Codex |
| README:35 非白名单群不读不记 | DESIGN:380 已说明默认只读，并由主审查核对主循环 | README 应列发言群、只读群、ignored_groups 三态，避免用户误判采集范围 |
| DESIGN Context:3 声称空仓库、无提交；10a:451 把一次一任务/无法收新消息列为当前缺失 | 现在有完整仓库；多槽、followups 和即时反思已实现 | 把历史比较标注基准日期，补一个“当前状态”入口，避免读者把历史描述当现状 |
| DESIGN:371 写命令 `dsh ... headless` | delegate.rs:389–398 实际运行 `--profile ... --json [--session-id ...] -`，模板名 headless 只出现在 profile 安装 | 当前调用契约写真实参数，把 headless 模板与旧子命令表述分开 |
| DESIGN:111、README:42–43 统称“原文快照 + SHA-256” | DSH 的 source.content 是模型返回摘录，research.rs:625–628 保存它；hash 钉住的是该摘录字节；fetched=yes 仅表示工具曾以 URL 为目标，不是抓取成功/原文逐字校验 | 将原始 HTTP/工具结果快照、模型摘录、搜索摘要标成不同证据等级；文档明确哈希只证明保存后不变；不将当前 DSH 摘录承诺为原文快照 |
| EVOLUTION:96 说审批后复核来源/HEAD/tree | 确实做，但仅核对候选自身；缺当前 release 版本关系 | 增加上线基线契约并修 HEO-01 |
| DEPLOY:21–43 两种发布共同使用 release 与 current.json | release 的两条写路径存在；current.json 只有手动路径写 | 修 HEO-02 与 HEO-06，统一实例/指针解析与交接协议 |
| DEPLOY:39 先校验 exe | 先停旧再哈希 | 修 HEO-04，文档应与真实步骤一致 |
| EVOLUTION:9、DEPLOY:61 明确 worktree/运行时根无 OS 权限隔离；DSH:228–230 明确 read-only 不限制读路径 | 源码只实施环境过滤与进程树限制，不是真正文件系统隔离 | 保留为已知边界，未把“缺 OS ACL”重复列作本次新漏洞 |
| DSH:261–290 列原始事件/工具调用审计和取消进程树 | delegate/process_tree 实现匹配；环境注册表 local-only 优先于 allowlist | 已对齐；Unix 分支只静态审查，本 Windows 审查不声称 Unix 运行已验证 |

## 覆盖与测试评价

已逐段阅读的实现：`src/work/{mod,board,research,resume}.rs`、`src/hands/{mod,delegate,process_tree}.rs`、`src/evolution/{mod,gate,process,restart}.rs`、`src/agli/evolution.rs`、`src/ns.rs`、`src/ns/secrets.rs`、`ops/{deploy,start-bot,start-stickers-web}.ps1`。同时核对 research/delegate 内嵌测试、gate/tests.rs、DSH fixtures、任务启动/完成/取消/看门狗相关 agli 调用段、main 的重启交接及 judge 的结果存储。根 README、DESIGN 相关章节、DSH、EVOLUTION、DEPLOY 均已核对。

已有覆盖较好的部分：拒绝非法实现者/不完整 director、先校验补丁实际路径再写工作树、L4/别名/穿越/符号链接拒绝、eval 必须实际执行、精确 tree 提交、Moldloft 来源变化阻止审批、缺少审查模型不自动放行进化、Windows 任务树取消/超时、环境 allowlist 与专用 DSH 密钥、owner followups 的研究步骤边界与有界重试、恢复时 sent/unknown 的提示与核实标记、旧看板兼容及卡片冻结。

未发现新的确定缺陷的区域：Namespace/Scope 基本语义、环境凭据 registry 默认拒绝、Windows kill-on-close Job Object、进化补丁路径/作者白名单、精确 tree 与父 checkout 隔离。此结论只针对已读代码，不证明所有外部工具/权限部署均正确。

本次额外验证：`pwsh -NoLogo -NoProfile -File target/audit-20261115/ops-mock-check.ps1` 返回 0，两条 CONFIRMED 输出。夹具仅使用审查目录的合成 JSON，并将 Get-Process、Stop-Process、Get-CimInstance、Get-FileHash、Invoke-CimMethod 替换为模拟；没有真实进程动作。

主要缺口：release 基线漂移、进化与脚本 PID 交接、脚本切换失败、完整 verdict→任务状态、编码执行中补充、custom data_dir。真实网络、实时 DSH profile、在线发布、进化重启和 Unix 生命周期未运行；不会用历史文档的测试数字证明当前部署行为。
