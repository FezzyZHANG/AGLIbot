# AGLIbot 仓库审查与设计对齐

审查日期：2026-11-15（Asia/Singapore）。源码基线：`0117d6f385b5f63004e72403d51fc8932aeeaf76`。初始主仓库工作区干净；本次仅修改文档并增加审查报告与合成验证夹具，未修改 `src/`、`config/`、人格种子、生产数据、运行实例或任何 live Moldloft ticket。

## 结论

主项目的模块分工、普通记忆过滤、执行器环境隔离和进化补丁校验已有实现及测试基础；问题集中在模块之间的边界。单个方法能正常工作，但自由文本派生过程丢失 scope/policy、并发读改写丢失更新、审查结果与任务状态脱节、进化与脚本各自维护运行身份。这些问题不能靠统一格式或补文档解决。

共合并为 **24 项发现：6 项 P1、18 项 P2**，包括 Moldloft 的 2 项重点后端发现。P1 表示应优先修复的隐私、权限、版本或控制边界问题；P2 表示确定的正确性、可用性、数据保留问题，或附有明确验证边界的安全缺口。没有证据支持 P0。已有测试全部通过不等于这些边界已受测。

**已完成的是审查和文档事实纠偏，核心缺陷尚未修复。** 遵守根 `AGENTS.md` 的写入限制，没有用本次审查绕过核心代码实现者或 L4 所有权规则。没有将建议自动变成已接受的设计决定，也没有更改线上版本。

**2026-11-17 更新。** A13（#92）和 A15（#93）已在源码与隔离运行实例中修复并复验。复验覆盖审查裁决、运行中追加要求、重载后的看板，以及完成态研究进入知识整理的条件。下文保留 11 月 15 日的基线发现。[后续验收记录](http://127.0.0.1:8787/graph?drydock=default&note=n-agli-ev-reading-work-20261117)

## 主人复核与后续工单（2026-11-15）

主人随后明确了 A01–A06 的处理方向，并要求维护 Moldloft。以下记录后续处置，不改写原审查证据，也不表示接受全部建议或已完成修复：

- **A01/A02**：[工单 #84](http://127.0.0.1:8787/graph?drydock=default&ticket=84)先复核脱敏、成长与对外主动行为的设计。保留抽象经验和有界人格成长，界定可识别细节、scope、processing_policy、来源与审查门槛；不默认采纳原报告的具体修法，也不把所有私人经历的影响都视为披露。未决问题为 `n-agli-q-growth-deidentification`，既有 `n-agli-dec-widen` 决策保留并关联该复核。
- **A03**：主人认可问题，[工单 #85](http://127.0.0.1:8787/graph?drydock=default&ticket=85)跟进审查项原子创建与稳定审批身份。
- **A04**：主人判断进化功能完成度明显不足，[原工单 #26](http://127.0.0.1:8787/graph?drydock=default&ticket=26)已从 Done 重开返工，覆盖上线基线、完整生命周期、发布/PID/预检衔接和重新验收；保留历史实现记录，不能只补一处祖先检查就再次宣称功能完成。
- **A05**：主人认可问题，[工单 #86](http://127.0.0.1:8787/graph?drydock=default&ticket=86)同时处理发送前撤销校验和旧记录覆盖管理决定。
- **A06**：主人要求独立超级开关，认为后端控制设计问题严重，可能需要重构。[工单 #87](http://127.0.0.1:8787/graph?drydock=default&ticket=87)先明确开关范围、独立取消/期限控制与迁移方案，再实施；尚未选择具体并发架构。

上述五个工单均为 Open / High；四个新工单为 #84–#87，#26 是重开。#5/#20/#36 保留历史完成状态并补关联说明。主人意见另存为 accepted decision `n-agli-dec-audit-p1-20261115`。本次维护经有版本校验的应用 API 完成并回读确认；没有启动实现、改动运行配置或重新启用主人单独搁置的 #27。

### 随后的全量工单复验

Moldloft 图版本从 164 更新到 165。原有 54 张 Done 工单逐单留下证据，48 张通过。#5 和 #28 因 A22 重开：启用仓库自带的本地聊天模型后，周成长和人格探针会选到无法满足条件的路由。#25、#30、#33、#80 回到 Open 补验，分别缺少只读工具限制的验收、三条排队消息得到一条完整回复的记录、Codex 同会话续跑的记录、在线词条更新进入对话的记录。DSH 的历史研究运行已找到，同一会话有两轮完成记录，末轮要求修改后任务仍进入 Done，给 A13 增加了运行证据。

Flow、Persona、NapCat 原适配里程碑已经整组归档，保留原工单号和依赖。#70 按七张子单通过的条件完成。QQ 的后续投递问题留在 #91。24 项审查发现均有工单跟踪，其中新建 12 张。脱敏设计仍待复核，独立超级开关与发布交接已有候选验收矩阵。逐项映射和归档清单见 [Moldloft 复验索引](http://127.0.0.1:8787/graph?drydock=default&note=n-agli-meta-audit-20261115)。

## 覆盖与证据

覆盖主项目全部 69 个 Rust 文件（约 25,347 行，含测试）、3 个运维脚本、内嵌贴纸页面、Cargo 清单、6 个配置文件、人格种子的加载路径，以及 README / DESIGN / CONFIG / DSH / EVOLUTION / DEPLOY 和相关研究记录。各分工逐文件阅读生产实现，结合测试和调用者核对；测试代码按完整执行加重点场景阅读，不声称逐行人工核对每个断言。

Moldloft 是独立子模块。本次额外审查其数据库/写锁、Drydock、提案、读取 basis、笔记、混合恢复、审阅渲染、HTTP/MCP 和项目安装边界；**没有完成子模块前端、打包、hooks 和全部维护命令的全面审计，也没有运行其完整测试集**。主仓库记录的 gitlink 为 `4903d28d1fb5d77fd13ee1dfe3bc4965876e5bdf`；受本地权限限制，未据此宣称子模块工作树 HEAD/dirty 状态已核实。

分项报告保留了更详细的触发条件、修复建议、验收用例与未发现问题的检查项；其中行号均指上述审查基线的源码，文档纠偏后的行号可能变化：

- [心智、社会与隐私](mind-social-privacy.md)
- [后端、存储、渠道与媒体](backend-store-channels.md)
- [执行器、任务、进化与运维](hands-evolution-ops.md)
- [Moldloft 后端补充](moldloft-backend.md)

## 优先发现

### A01 [P1] 私人日记衍生文字进入所有会话的全局提示

位置：`src/mind/growth.rs:117`、`:169`、`:70`；`src/mind/context.rs:59`；`src/agli.rs:2276`。对应分项 MS-01。

周成长从私人日记生成 `voice_note` / interests，只检查长度和数量，不保留来源 scope，也不经过 Widen 审查。随后这些文字被加入全局 L1，任意群聊和远程聊天模型都会收到。普通 `privacy::recall` 过滤正确，也挡不住这条绕过召回入口的路径。

已用原生产模块和 stub 模型复现：私人日记 canary 被模型作为普通语气备注返回，`weekly` 成功，`build_l1` 包含 canary，审查队列为空。它证明代码允许该信息流，不表示已在线观察到泄漏，也不依赖模型恶意行为。

修复方向：把数值风格增量与自由文本分开；文本携带来源权限，进入全局提示前必须满足公开范围升级规则。验收必须捕获群聊的模型输入，不能只检查最终回复是否说出了秘密。

### A02 [P1] 主动联系的 reason 把私人思考内容带入主人会话

位置：`src/agli.rs:2329`、`:2369`、`:2378`；`src/mind/commitments.rs:123`；`src/mind/decide.rs:118`、`:135`。对应 MS-02，静态完整调用链确认。

solo 决策可读取所有人的临近承诺，并在本地模型可用时读取 SelfOnly lessons；它返回的自由文本 `reason` 被原样插入主人会话。目标听众校验只判断“能否联系主人”，没有判断 reason 的来源是否允许向主人披露，也没有把 LocalOnly 约束带到下一次 chat 调用。例如，正常根据 Alice 的私聊承诺作出联系决策，就可能把 Alice-only 信息交给主人或远程模型。

修复方向：内部决策理由不作为外部聊天正文。行动返回枚举或来源引用，真正生成面向主人的内容时重新按目标听众和处理权限检索；确需扩大范围的内容先走 Widen。验收覆盖 Alice-only 承诺和本地经验两种 canary。

### A03 [P1] 并发审查编号冲突会覆盖待决项和批准 payload

位置：`src/mind/judge.rs:115`；`src/store/jsonl.rs:285`；`src/store/mongo.rs:357`。对应 MS-03。

`next_review_id` 和 `save_review` 分开执行，两个后端都用当前最大编号加一；保存同编号是覆盖。实际并发来源存在：后台 research 作业与主循环转告/反思可以同时生成审查项，默认一个作业也足够触发。主人看到的编号可能随后对应另一条 payload。

已复现生产 Store API 允许的确定交错：两个调用各取到编号 1，再分别保存不同的待决事项，最终只剩后者。这是分配/保存窗口的可执行证明，没有假称做过生产压力测试。

修复方向：原子分配并插入审查项；更新已有审查项另走版本校验。验收用 barrier 固定交错，检查两条 subject/payload 均保留，而非只检查单线程编号递增。

### A04 [P1] 进化从过期基线发布可隐含回滚在线修改

位置：`src/evolution/gate.rs:313`、`:344`、`:469`、`:575`。对应 HEO-01，静态完整调用链确认。

当前 release 为 B，候选却可从旧 A 构建；闸门只核实 A 是有效 commit、候选自己的 tree 和来源未变，提交父节点仍是 A。审批看到的 diff 也是相对 A 的小补丁，实际部署 A+patch 会丢失 B 的其他修改，甚至把 A 之后新增的闸门保护一并退回。手动部署已有祖先检查，进化没有同等检查。

修复方向：准备候选时固定当前 release 身份，校验候选包含该提交；审批与交接前再核对未漂移，并展示相对在线版本的完整差异。用 A→B、两次未合并进化、检查后 release 改变三种合成 Git 场景验收。

### A05 [P1] 主人撤销贴纸状态或范围不能可靠生效

位置：`src/agli.rs:1578`、`:1596`；`src/mind/expression.rs:27`；`src/mind/media.rs:434`；`src/stickers_web.rs:230`。合并 B01/B02，避免重复计数。

有两条独立成因。第一，候选 Offer 捕获旧记录，模型慢调用期间主人禁用或收窄 scope，发送前却不重读，仍可跨群发送。第二，worker/发送计数/Web/recaption 都整条读改写；旧的 sighting 写回会把刚保存的 Disabled/Origin 恢复成 Active/Any，删除也可被旧 upsert 重新创建。Mongo 的单文档原子写并不能阻止这种旧值覆盖。

第二条已用生产 JSONL Store 复现：主人保存 Disabled/Origin 后，worker 仅增加 sighting 的旧记录写回，最终恢复 Active/Any；第一条由完整发送调用链确认，未真实发图。

修复方向：发送前重新核验状态/范围，另以字段更新、原子计数或版本检查保护管理操作。验收覆盖检索后禁用、收窄、删除，以及并发计数/再次见图；两类成因都要修。

### A06 [P1] 慢模型调用阻断冻结、停止与任务级超时检查

位置：`src/main.rs:158`、`:165`；`src/mind/llm.rs:283`；`src/agli.rs:729`、`:2054`、`:2153`。对应分项 ROOT-C2。

外层 `select!` 选中 work 后，在分支内部等待完整 `Agli::handle`；此时不再轮询 Ctrl+C 或新命令。模型 HTTP 请求可等 600 秒，升级与夜间串接更久，`/stop` / `/freeze` 和 Tick 都排队；watchdog 只在 Tick 处理时检查。子进程自身的单次 timeout 仍有效，不能把问题夸大成所有超时失效。

DESIGN 10a 已记录聊天串行，10c 暂缓全双工工具，因此串行架构不是未披露的新问题；但它使第 9 节“立即冻结”的控制验收不成立。最小修复是让控制、退出信号与任务期限监测独立于长生成，普通会话可以继续串行。验收用悬挂的本地模型服务，要求在请求未返回时完成有界取消和进程清理。

## 其余发现

每项的完整证据和验收见分项报告；A21/A22 在下节补充。本表中的“静态”表示已确认实际调用链与可达交错，没有声称做过在线故障注入。

| ID | 等级 | 具体问题与触发影响 | 主要位置 | 证据 / 分项 |
|---|---|---|---|---|
| A07 | P2 | JSONL 的实例内 Mutex 不保护独立 Web/bot，固定临时文件及整文件重写会使不同 hash 的更新也互相丢失 | `src/store/jsonl.rs:122`、`:139`、`:335` | 静态；B03 |
| A08 | P2 | import-jsonl 只迁移旧十类数据，漏掉权威 lexicon/media；校验仍能成功，禁用/范围/手工描述丢失 | `src/store/import.rs:17`、`:37`；`src/store/jsonl.rs:44` | 静态；B04。向量是缓存，可有意重建；权威文字不能这样省略 |
| A09 | P2 | 先保存图片后描述，预算拒绝或失败不建记录；清理只枚举 records，孤儿图片永久逃过七天清理 | `src/mind/media.rs:428`、`:528` | 静态，已有失败夹具确认无记录文件；B05 |
| A10 | P2 | QQ 合并转发任何错误都退回普通分片，已送达但回执丢失时重复发送 | `src/channels/qq.rs:237`、`:249` | 静态；B06。仅明确拒绝才能安全 fallback |
| A11 | P2 | QQ 超时请求在重连后仍可能迟发，pending 回调在连接不断且无回执时无限留存 | `src/channels/qq.rs:222`、`:416`、`:467`、`:487` | 静态；B07。期限需覆盖入队、发送和回执 |
| A12 | P2 | 进化成功的新 PID 不写 current.json，下一次正常 -Restart 拒绝接管；deploy 已先换 release 指针 | `src/evolution/restart.rs:156`；`ops/start-bot.ps1:12`；`ops/deploy.ps1:64` | 真实脚本+模拟进程复现；HEO-02 |
| A13 | P2 | research/code 的最终 revise/defer/owner 仍返回 ok=true，任务被标 Done；revise/defer 审查还默认关闭 | `src/work/research.rs:48`、`:750`、`:789`；`src/agli.rs:1994` | 静态；HEO-03。仅停止尝试不应等同通过验收 |
| A14 | P2 | -Restart 先停健康实例再检查下一 exe，候选缺失或哈希不符立即造成停机 | `ops/start-bot.ps1:12`、`:29` | 真实脚本+模拟进程复现；HEO-04 |
| A15 | P2 | code 开始后收到的 followup 不在完成前复读，已接受要求静默遗漏且任务关闭 | `src/work/research.rs:768`、`:789` | 静态；HEO-05。不要求打断执行器，但须处理或明确保留未处理状态 |
| A16 | P2 | Rust 跟随自定义 data_dir，ops 固定 data，release/PID 和防回滚检查读到不同位置 | `src/config.rs:482`；`ops/deploy.ps1:24`；`ops/start-bot.ps1:29` | 静态；HEO-06 |
| A17 | P2 | add_fact 先改缓存后保存，保存失败后同进程重试按缓存去重跳过，水位可推进而事实从未落盘 | `src/social/people.rs:164`；`src/mind/reflect.rs:157` | 静态；修订后的 MS-04。该项独立于已延期的崩溃去重 |
| A18 | P2 | digest 构造时先标 notified，无 owner 目标或发送确定失败后，仍开放的待决项永久不再进入摘要 | `src/mind/judge.rs:161`；`src/agli.rs:2188` | 静态；MS-05 |
| A19 | P2 | 夜间工作只由入睡边沿触发，任一早期可恢复失败使整个当晚后续阶段不再执行 | `src/agli.rs:2155`、`:2255` | 静态；MS-06。不是要求无限重试，可按日期/阶段有限退避继续 |
| A20 | P2 | last_seen 落盘节流与每条消息刷新的缓存时间混用，持续每分钟发言可使存储时间一直停在首条 | `src/social/people.rs:65` | 静态；MS-07 |
| A21 | P2 | 长期潜水群 unseen 无界累积，triage 反复提交全部旧消息，最终放大内存/花费并撞上下文上限 | `src/agli.rs:1109`、`:1212`、`:1239` | 静态；ROOT-C1 |
| A22 | P2 | 默认本地模型可用后，日记/成长及人格探针反而无合法路由 | `src/mind/models.rs:88`；`src/mind/reflect.rs:412`；`src/mind/probes.rs:47` | 生产 Router + 当前配置探针复现；下节详述 |
| M01 | P2 | Moldloft apply 后按已应用图重算并覆写原 review diff，原 Before/After 消失，amendment 还可能显示反向差异 | `Moldloft/src/tickets/proposals.py:313`、`:414` | 真实临时 SQLite + 生产 renderer 复现；Moldloft M01 |
| M02 | P2 | Moldloft REST 只比 Origin 与请求 Host，无全局 Host allowlist；任意 Host 配同 Origin 可通过，存在 DNS rebinding 缺口 | `Moldloft/src/tickets/app.py:165` | 真实 middleware 复现 204/403；没有浏览器攻击验证，浏览器网络限制可能另行缓解；Moldloft M02 |

### A21 的资源增长边界

可发言群每条消息都 push 进 unseen；triage 判断“不说”只复位 floor 计数，不清除这些行。之后每隔 check_every 又 clone/render 全量历史。一个正常活跃、无需插话的群也能触发，累计模型输入可近似二次增长；后来被 @ 仍要带上全部积压，导致直接回答也失败。

修复应给每群条数/时间/字符设固定窗口，并在已读但不说时推进游标；原始记录仍由 Store 保留。验收持续灌入数千条、stub 始终 speak=false，检查缓冲和每次请求大小有界，最新 @ 不被丢弃。

### A22 的本地模型与场景能力冲突

`has_local` 只表示任意一个本地聊天模型通过启动可达性检查。仓库本地 `qwen3:8b` 的 Q=5，而 consolidate 要 Q≥6。只要该模型真的安装并列在 `/models`，write_journal / growth 就根据 has_local 选择 LocalOnly，随后必然无候选；write_journal 的错误又使 nightly 后半部分直接退出。

人格探针也按 has_local 改用 LocalOnly，却调用固定远程 Gemini 且要求 vision 的 chat；即使提高本地 Q 也不能解除这个冲突。只运行用于 bge-m3 的 Ollama、没有 qwen3:8b，不会触发 has_local。

第四个隔离探针读取当前 models/scenarios，证明 consolidate/chat 的 Any 可选而 LocalOnly 不可选。修复要按场景判断能力、为探针提供合适路由，并在没有合法本地模型时安全跳过受限材料；不能把已混入 SelfOnly 的正文直接回退给云端。

## 设计、文档、实现对齐

本次对 README、DESIGN、CONFIG、DSH、DEPLOY、EVOLUTION 做了事实纠偏。以下把已改文档与仍需实现/决策的内容分开，避免通过降低设计要求来宣称修复。

| 主题 | 核实的实现状态 | 本次对齐结果 / 未完成部分 |
|---|---|---|
| 启动与模型 | chat/caption 固定 Gemini，format 固定百炼 DeepSeek Flash，evolve 固定 Opus | 删除“只有百炼就覆盖所有场景”；CONFIG 补 model 字段、format 场景和精确必需场景名单；未验证真实 provider 在线 |
| 预算 | 未固定的一般路由降档；固定非豁免拒绝；部分后台停止；DSH 独立 | 改正“所有场景全局硬限”描述，保留 token/金额硬上限为未完成目标；没有修改 L4 |
| 群处理 | 发言白名单之外默认只读，仍采集/采样；ignored 停止新增及分群提炼/归档 | README/CONFIG 区分三态；历史 ignored 群产物并未从所有日记/互惠/语义召回路径撤回，不能宣称彻底擦除 |
| 图片与贴纸 | 已能直接看图、后台描述和 QQ 发贴纸；ASR/TTS 仍研究 | 更新 DESIGN 的过期“尚未接入”；保留研究文档里的历史方案，不将其旧阶段当现行授权 |
| 普通 scope 召回 | recap、fact、episode、semantic recall 先过滤；群记忆到成员私聊是已接受的单向扩展 | 基础过滤有测试；A01/A02/A05 是其他入口绕过，不能靠这些单元测试宣布全链路隐私成立 |
| processing_policy | SelfOnly 多处推导 LocalOnly；Providers 值类型存在，数据级 namespace override 未完整接入 | 记作尚未落实设计能力。Widen reviewer 默认 Any 的条件性弱化需保留源 policy；未重复计入所有普通私聊为违规 |
| 本地反思 | has_local 与场景能力不等价 | CONFIG 明示 A22；不能用默认无本地的 stub 测试证明有本地时正常 |
| 词库与配置热加载 | social 运行设置可热加载；aliases 仅一次启动种子，之后 Store 为权威 | 补正“全部字段实时生效”的表述和 aliases 字段；其他文件仍需重启 |
| 当前协议与价格字段 | Gemini provider 已走 Messages；部分模型显式 cache_write | CONFIG 更正 reasoning_effort 的协议条件与 cache_write 现状；未重新核价，也未声称路由估值等于账单 |
| 状态文件 | mind/state 是种子；实际状态写 data_dir/state | 设计树仍需按“目标目录/运行权威/种子”整体整理，不能把原树当精确文件清单 |
| 执行器 | 缺键默认 Codex；仓库显式研究 DSH、编码 Codex | README/DSH 已区分缺键默认与仓库配置；在线配置未检查。DESIGN 历史比较已标日期，避免把已实现多槽/followups/watchdog写成当前缺失 |
| 来源快照 | DSH 保存的是模型返回摘录；SHA-256 钉住所存字节，不证明网页原文或抓取成功 | README 更正；研究证据应区分原始抓取、摘录、搜索摘要并进行独立来源验证，这项能力未被文档更新“实现” |
| 发布与重启 | 候选自身 tree 校验存在；在线 release 基线、PID 交接、预检顺序不完整 | EVOLUTION/DEPLOY 标明 A04/A12/A14/A16；没有实际部署或重启 |
| 知识体系 | 能从 Done research 生成知识卡/信誉/立场，暂未见聊天 L1/L2 读取这些卡片 | 区分“写了知识文件”与“聊天能检索使用”。A13 还可能把未通过审查的结果送入 Done 后续路径 |
| OS 隔离 | worktree、环境白名单、Windows Job Object 有实际作用；L4 文件 ACL/独立 OS 用户隔离未部署 | 沿用已明确的部署边界，不把它重复包装成本次新发现 |
| 恢复与去重 | 反思分步写结果和水位，崩溃重跑可重复记忆；夜间排队结果有内存状态 | DESIGN 8 已将完整去重/跨存储恢复列为后续，不计新增缺陷；A17 是同进程失败重试丢事实的独立错误 |
| Moldloft 人类采信 | MCP.md 禁止 agent 提议 adopted，但 reducer/测试允许 pending adopted proposal | 仍须主人 apply，不是审批绕过；需要决定契约含义。本次未替用户作产品决定，也未改子模块源码/契约 |

## 验证结果

工具链：rustc/cargo 1.95.0。所有 Rust 检查使用 `--locked --offline`，忽略的真实集成测试未强行启用。

| 检查 | 实际结果 | 说明 |
|---|---|---|
| cargo build --locked --offline | 通过 | 主程序正常编译；环境有增量缓存硬链接退回复制的警告 |
| cargo test --locked --offline --quiet | **374 通过，0 失败，17 ignored** | 391 项；排除环境限制后的完整基线结果 |
| cargo clippy --locked --offline --all-targets -- -D warnings | **失败** | 7 处诊断：speak 参数过多、stickers 排序、channels test module 顺序、Telegram 测试忽略 read 长度、media/vision 三处 clone；不把格式/风格问题冒充运行漏洞 |
| cargo fmt --all -- --check | **失败** | 66 个文件、1679 处 diff 块；未自动格式化核心代码 |
| Rust 独立审查探针 | **4 通过，确认缺陷存在** | 原生产模块 unchanged include；stub/合成数据；A01/A03/A05(旧值覆盖)/A22。通过表示成功复现现状，不能当修复后的回归通过 |
| 真实启动脚本+模拟进程 | **2 个问题确认** | A12、A14；未启动、停止或查看真实 bot 进程 |
| Moldloft 临时 SQLite / middleware 探针 | **结果保存** | M01 diff 丢失；M02 matching Host/Origin=204、不同 Origin=403；pending adopted 仍未创建笔记 |
| Moldloft project.py check --project . | ready，issues=[] | 只证明安装检查；live_host_activation=not_verified，不等于在线服务与 LAN 可用 |

受限环境首次运行有临时目录替换权限错误，改用工作区临时目录后剩本地 socket 权限错误；允许合成测试的本地连接后全套通过。上述 374/0/17 是最终可解释结果，**不把前两次环境失败计作仓库缺陷**。

未进行：真实模型付费调用、真实群聊发送、在线数据审计、Mongo 集成/压力故障测试、真实发布/进化重启、Unix 进程生命周期、浏览器 DNS rebinding、依赖漏洞数据库扫描。未读取 `.env` 值，未发送消息给他人。

可重复执行的夹具：

```powershell
# 从仓库根执行；只写 target 内合成数据，Rust 探针无真实模型/渠道请求。
pwsh -NoProfile -File docs/reviews/2026-11-15/run-probes.ps1
pwsh -NoProfile -File docs/reviews/2026-11-15/ops-mock-check.ps1
& .tools/moldloft/Scripts/python.exe -B docs/reviews/2026-11-15/moldloft-probes.py
```

生产代码修复后，上述“确认现有缺陷”的探针应改成拒绝错误行为的回归测试，不能原样用于验收新实现。原完整输出保存在本机 `target/audit-20261115/`；可共享的精简证据见 [EVIDENCE.txt](EVIDENCE.txt)。

## 建议修复顺序与验收门槛

1. **先闭合隐私与审批边界**：A01/A02 的派生文本权限、A03 审查原子插入、A05 贴纸撤销与并发更新；每项先以合成 canary/barrier 证明旧行为，再检查不泄漏、不覆盖。
2. **统一运行版本与控制身份**：A04 release 基线，A12/A14/A16 发布/PID/预检协议，A06 独立控制与期限；用临时 Git 仓库、挂起服务和模拟进程验收，不直接拿在线 bot 试错。
3. **让结果和持久化状态一致**：A13/A15 的任务状态与补充，A17/A18/A19 的失败处理，A07/A08 的存储与迁移；补跨模块状态矩阵和双实例交错，保留明确的 Unknown 发送语义。
4. **补资源与运维边界**：A09/A10/A11/A20/A21/A22，M01/M02 分别在其独立子模块走修复；随后统一格式、清理严格检查错误，并固定工具链/持续检查入口，防止文档与测试继续漂移。

根仓库目前未见 CI 工作流或固定 Rust 工具链文件；没有新增隐藏规则或自行格式化全仓库。首要维护性建议是把 `agli.rs` 的大段测试与控制/交付/群会话职责逐步拆开，再把“通过审查”“确实送达”“已落盘”“当前运行版本”变成独立、可验证的状态，而不是复用一个 bool 或自由文本。

这份清单是可评审的修复输入，不代表核心改动已获实现、上线或 L4 修改授权。
