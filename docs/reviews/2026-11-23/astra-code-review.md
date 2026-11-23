**结论：建议暂缓将 `463363c` 部署到已有数据的运行实例。** 六个提交均已按父提交逐项核对。目标版本构建成功，仓库测试 **309 通过、0 失败、10 忽略**，严格 Clippy 通过。发布阻断项集中在升级和并发边界：BSON 迁移会抹去记忆归档关系，旧任务补充不会导入，`/stop` 可以漏掉已经进入启动流程的任务，研究工作器也有越过任务目录的文件写入路径。阶段 0 还删除了实际接入的功能和 owner-only 的进化闸门。另用四个仓库外离线探针确认了旧任务补充遗漏、停止竞态、毫秒水位漏读和 A20 落盘失效。本次未修改 `C:\dev\AGLIbot`，未启动 bot、连接 MongoDB 或调用真实渠道及模型服务。

### 发现

行号均对应 `463363c`。标为“继承”的问题在 `590a346` 已存在，列出是因为它涉及本次声称关闭的审查项或用户指定的隐私边界。

| ID | 级别 | 阶段 / 提交 | 文件:行 | 问题 | 具体失败场景 | 建议修复 |
|---|---|---|---|---|---|---|
| R01 | P1 | 2 / `35a51f0` | `src/store/migrate.rs:176`、`:227`、`:284`；`src/store/mongo.rs:326` | 旧 `Episode` 经新结构反序列化再原地替换，丢弃 `sources`。迁移测试明确断言该字段消失。新 `episodes()` 全表返回，旧版会过滤已归档原文。 | 一份摘要归档了三条旧记忆。升级后四条都可再次参与召回，数据库也失去了说明哪三条已被归档的关系。 | 保留来源 ID 和归档状态。用含摘要和原文的旧库夹具对比迁移前后的有效召回集，并验证重复迁移。 |
| R02 | P1 | 5 / `463363c` | `src/work/board.rs:56`、`:64`；`src/store/mod.rs:350`；`src/work/mod.rs:155` | 看板导入只读 `board.yaml`。旧版把主人补充的正文写在 `tasks/<id>/followups.md`，新字段缺省为空，工作器从新字段构造任务要求。 | 主人已要求“改用方案 B”，升级后 `/run` 仍按原标题执行，任务可以通过审查。离线探针已复现导入结果为空。 | 幂等导入旧补充，结合旧审查的 `reviewed_followups_bytes` 恢复消费位置。不能可靠对应的补充保留为未处理。 |
| R03 | P1 | 1、5 / `6808481`、`463363c` | `src/bot/jobs.rs:50`、`:62`；`src/bot/ingress.rs:77`；`src/control.rs:152` | `/stop` 只扫描已注册的 jobs，未与任务准入同步。任务置为 `Active` 和注册可取消句柄之间还有异步审计写入。 | `/run` 已把任务置为 `Active`，此时 `/stop` 返回“现在没在做任务”。审计随后完成，工作器照常启动。离线探针固定了该交错。 | 准入时先登记可撤销的 attempt，让 `/stop` 同时撤销启动中和运行中的 attempt。注册执行器前复核撤销状态。 |
| R04 | P1 | 1 / `6808481`，2 / `35a51f0` | `src/control.rs:199`、`:212`、`:223`；`src/bot/ingress.rs:82` | 冻结标志读取出错或类型异常时按 `false` 启动。执行 `/freeze` 又要先取得跨异步准入操作的锁，并成功写 Mongo，之后才关闭准入、取消工作。 | 已冻结实例重启时恰好读 KV 失败，可以恢复主动工作。运行中 Mongo 阻塞或写入失败时，主人发出的冻结也不能及时停下已有任务。 | 读取冻结态失败时保持禁用或中止启动。收到冻结先关闭本地准入并取消，再分别报告持久化和终止结果。 |
| R05 | P1 | 5 / `463363c` | `src/work/board.rs:87`；`src/work/research.rs:112`、`:132`、`:198`、`:207`；`src/work/coding.rs:38` | 工作器检查任务目录或 `scratch` 本身，未完整核验可信根到目标的路径。研究工作器还直接读写既有的 `plan.md`、`report.md`。 | 恢复一个带文件链接的旧任务时，bot 进程可读到任务外文件并送入综合提示，或把报告写到任务外。父目录是 Windows junction 时，仅检查末级目录也不足。 | 对数据库任务 ID 和规范化根做统一校验，拒绝祖先和目标文件的重解析点。产物使用可信目录中的新文件写入，再安全替换。 |
| R06 | P1 | 1、5 / `6808481`、`463363c` | `src/mind/llm.rs:103`；`src/work/research.rs:82`、`:95`；`src/mind/models.rs:343` | Anthropic 兼容请求省略输出 schema，而新的研究规划提示也未告诉模型必须返回 JSON 或四个必填字段。仓库 `research_plan` 的质量优先路由可选中走此协议的 Gemini。 | 模型按提示返回一份正常的中文研究计划，解析 `ResearchPlan` 失败。若当前已是最高 Q，连升级重试对象也没有，任务在取证前转为等待。 | 对不发送原生 schema 的提供商，把结构约束注入模型实际收到的提示。用真实请求构造器和本地响应夹具验证。 |
| R07 | P1 | 2、4 / `35a51f0`、`f949d6b` | `src/store/mod.rs:412`；`ops/deploy.ps1:69`；`ops/start-bot.ps1:26` | 正常发布流程写入新指针并停止旧 bot，新版本随后才检查旧 Mongo schema 并拒绝启动。发布脚本没有安排离线迁移，也没有协调仍在运行的旧贴纸页面。 | 对健康的旧版 Mongo 实例直接按部署文档发布，旧进程先退出，新 bot 因要求 `migrate-bson --apply` 而退出，指针却已指向新版本。 | 切换前执行只读 schema 预检。需要迁移时，明确停止两个写入进程、备份、迁移、校验，再切换指针并验证服务就绪。 |
| R08 | P1 | 2、4 / `35a51f0`、`f949d6b` | `src/main.rs:26`、`:68`；`src/store/migrate.rs:73`、`:158` | `migrate-bson` 只处理 Mongo 内的旧 `record`。旧 JSONL 导入入口已删除，主程序又把未知子命令当作正常启动。 | 管理员运行旧版 `aglibot import-jsonl --from … --dry-run`，在配置齐全的环境会进入 bot 启动流程。旧 `aglibot stickers-web` 调用也有同样问题。 | 显式解析子命令，未知或退役命令立即非零退出。为 JSONL 用户提供可验证的离线迁移路径，包含 media 和 lexicon。 |
| R09 | P1 | 0 / `0fe9648`；1 / `6808481` | `src/bot/conversation.rs:37`、`:43`、`:66`；`src/mind/context.rs:115` | **继承的跨听众隐私问题。** 活动任务标题被直接放进每个聊天的提示。任务卡虽有主人私聊限制，这个入口没有听众检查。 | 主人在私聊创建带人名或私事的任务，任务运行时有人在 QQ 群点名 bot，群聊模型会收到完整任务标题。无需模型在回复中复述，披露已经发生。 | 非主人会话只传“有任务进行中”。允许显示标题的会话也按任务来源校验听众和处理策略。 |
| R10 | P1 | 0 / `0fe9648` | `AGENTS.md:15`；`src/evolution/mod.rs:5`；`src/bot/commands.rs:128`、`:226`、`:247` | L4 的 `src/evolution/gate.rs` 被删除，原 `/evolve`、审批、发布与重启入口变成占位。旧 `Evolve`、`Widen`、人格探针审查项仍可被通用代码标为 `Approved`。 | 主人批准升级前遗留的编号，收到“通过。记下了”，相应发布、范围升级或重设基线均未执行，编号也不能再处理。 | 由 owner 裁定闸门退役或恢复。保留历史审批的真实语义，对已退役类型显式拒绝或迁为可追踪的终止状态。 |
| R11 | P1 | 2、3 / `35a51f0`、`31dab7b` | `src/mind/media.rs:465`、`:469`、`:508`；`src/store/mongo.rs:237`、`:463` | A05 仍有删除竞态。worker 先读到已有记录，管理端随后删除，字段更新返回 `None` 后，worker 将其当作新图描述并整条 upsert。 | 主人在一次已经开始的 sighting 中删除图片，慢描述完成后同一 hash 的记录重新出现，甚至可能只恢复记录而缺少文件。 | 区分“首次发现”和“已观察版本被删除”。用版本或删除标记阻止旧作业重建；首次创建使用条件插入。 |
| R12 | P2 | 0、3 / `0fe9648`、`31dab7b` | `src/bot/scheduler.rs:223`、`:250`；`src/mind/context.rs:61`；`src/bot/mod.rs:312` | 阶段 0 删除的多项功能有生产调用。旧 nightly 会做知识整理、关系更新、人物卡、周成长和人格探针；旧聊天会应用 drift、语义召回，调度会检查到期 watchlist。 | 已有 `drift.yaml` 的人格升级后退回种子表现；已登记的跟进主题不再触发；完成的研究不再进入原知识整理流程。 | 按功能列出保留、退役和数据处置，请 owner 确认产品取舍。需要保留的功能恢复到真实调用入口。 |
| R13 | P2 | 1 / `6808481` | `src/mind/speak.rs:142`、`:154`；`src/bot/conversation.rs:159`、`:187` | Speaker 保住了供应商缓存块，也永久保留了原始草稿。旧版 `settle` 会把历史校正为实际发出的内容。 | 格式化步骤替换或撤回了草稿，下轮模型仍看到未发送的原文，可能引用用户从未看过的话或内部任务标记。 | 区分供应商缓存前缀和已交付对话。草稿与送达结果不一致时重建受影响的会话，或追加明确的送达校正。 |
| R14 | P2 | 4 / `f949d6b` | `src/channels/telegram.rs:121`、`:130`；`src/channels/urls.rs:55` | Telegram `getFile` 使用配置好的代理，实际文件下载改用 `.no_proxy()`。旧版两步都用 Telegram 客户端。 | 只能经 `127.0.0.1:7890` 访问 Telegram 时，聊天正常、文件 ID 也能解析，图片和文档正文却下载失败。 | 为 Telegram 可信文件端点保留配置代理，重定向继续执行目的地检查。增加覆盖真实下载分支的代理夹具。 |
| R15 | P2 | 5 / `463363c` | `src/bot/jobs.rs:46`、`:50`、`:68`；`src/work/board.rs:166`；`src/store/mongo/tasks.rs:91` | Mongo 的快照比较能防字段覆盖，但状态转换不校验期望前态。`Active` 已写入后，日志或审计失败也没有补偿。 | 审计写入失败，任务留在 `Active`，没有工作器，手动 `/run` 被拒绝，直到重启恢复。两个实例也可能先后把同一任务认领为活动任务。 | 在一次条件更新中认领允许的前态并生成 attempt ID。启动失败可回收，完成和停止只结算同一 attempt。 |
| R16 | P2 | 2 / `35a51f0` | `src/store/mongo.rs:20`、`:279`；`src/mind/reflect.rs:168` | BSON 把时间截到毫秒，反思水位仍是单个时间戳，续读只查严格大于它的消息。旧 Mongo 路径保留 JSON 中的更高精度并作精确比较。 | 两条消息在同一毫秒内先后写入，第一条之后已推进水位，则第二条仍在消息库里，却不会再由该水位读出。离线探针在采用相同精度的 MemStore 上复现。 | 改用 `(ts, _id)` 稳定游标，并为旧时间水位提供兼容读取。 |
| R17 | P2 | 3 / `31dab7b` | `src/bot/commands.rs:153`；`src/bot/ingress.rs:447` | 自动摘要已改成发送成功后标记，手动 `/digest` 仍在返回正文前写 `notified=true`。 | 主人手动拉摘要时发送失败，条目继续 Open，却从下一次自动和手动摘要消失。 | 让命令返回正文和待确认 ID，交付成功后统一标记。 |
| R18 | P2 | 2 / `35a51f0` 声称涉及 A20 | `src/social/people.rs:66`、`:87`；`src/store/mongo.rs:303` | **原 A20 未修。** `last_seen` 每条消息都刷新缓存，落盘阈值也拿这个缓存时间比较。`spent_since` 聚合解决的是另一件事。 | 离线探针每分钟送入一次发言，持续一小时。缓存推进了一小时，重新读取存储仍是首次发言时间。 | 分开记录最近观察时间和最近成功持久化时间，用后者做节流。 |
| R19 | P2 | 1、4 / `6808481`、`f949d6b` | `src/main.rs:123`；`src/channels/qq.rs:641` | 主循环前引入无界 relay，慢模型期间普通消息可以持续占用内存。QQ 更上游的有界缓冲满时直接丢最旧事件，尚未区分主人控制命令。 | 长调用期间持续进群消息，进程内积压突破原通道容量。若堵塞发生在 QQ 富化层，已收到的 `/freeze` 也可能在到达控制平面前被挤掉。 | 独立交付控制事件，普通流量采用有界队列和明确的过载策略。 |
| R20 | P2 | 5 / `463363c` | `src/config.rs:323`、`:337`；`src/work/research.rs:150`；`config/runtime.yaml:32` | `research: dsh` 被静默解释为 native；原专用 profile、home 和授权密钥不再生效，改读 `DEEPSEEK_API_KEY`。 | 按现有部署文档只配置专用 DSH 密钥的实例可以启动，首个需要搜索的研究才报缺少凭据。若另有通用 key，则改用另一套授权和账目。 | 显式迁移执行器配置和凭据来源。升级预检报告实际后端、所需变量名和失效字段。 |
| R21 | P2 | 5 / `463363c` | `src/work/research.rs:130`、`:159`；`src/work/research/sources.rs:93` | 审稿要求“修改后再审”不会独自触发重拟检索计划。旧 DSH 的 `01-dsh.md` 等来源文件也因文件名不是纯数字而被跳过。 | 审稿人要求补查新的一手来源，下一轮仍搜原来的词；恢复的旧任务还可能有报告却显示零份已取回来源。 | 把需要补证的审稿意见纳入重规划条件，并迁移旧来源文件、取证状态及引用关系。 |
| R22 | P2 | 5 / `463363c` | `src/hands/reader/web.rs:163`；`src/work/research/sources.rs:196` | 抓取 API 没给目标站状态码时，代码拿服务自身的 HTTP 状态填充目标状态，并写入来源记录。 | 服务返回 200 和正文、未返回上游状态，报告证据却写成目标网页 `HTTP_STATUS: 200`。 | 分开保存服务状态、上游状态和是否取得正文。上游状态缺失时保留 unknown。 |
| R23 | P2 | 2 / `35a51f0` | `src/social/privacy.rs:65`；`src/store/mongo.rs:326`、`:351`；`src/bot/mod.rs:188` | 移除旧缓存后，聊天召回从 Mongo 拉回全部 episodes 再本地筛选；常规事件结束时，有主人路由的紧急提示检查还会读取完整 reviews，包括研究证据 payload。 | 数据库中已积累多年记忆和长研究报告时，普通消息也会触发与全库大小成比例的传输和反序列化。 | 对 namespace、参与者及待推状态做服务端查询和投影；摘要检查只取必要字段。 |
| R24 | P2 | 4 / `f949d6b` | `ops/deploy.ps1:22`、`:50`；`ops/start-stickers-web.ps1:29` | 部署脚本允许指定旧 commit，但一律要求构建新 `stickers-web` bin；新启动脚本也只认新指针字段。 | 按原有 `-Commit 590a346 -Force` 回滚，Cargo 因旧提交没有该 bin 而失败，即使旧版主程序仍可构建。 | 按目标提交识别单、双二进制布局，兼容旧 release 指针，并分别说明数据库回退条件。 |
| R25 | P3 | 1、3、4 / `6808481`、`31dab7b`、`f949d6b` | `README.md:21`；`Cargo.toml:37`；`docs/DEPLOY.md:41`、`:53`；`src/social/skill_lines.rs:39` | README 仍给双 bin 且无 `default-run` 的项目写 `cargo run`。部署文档仍写先停后验和旧贴纸子命令。`SkillLines` 实际读取 `mind/self/canned_replies.yaml`，缺失时回落到编译内默认值。 | 新开发者按 README 无法选定启动目标，运维照旧说明调用错误入口，修改 `config/` 也不会改变技能台词。 | 指定默认 bin 或修正启动命令，更新部署、固定 debounce 和技能台词的真实配置位置。 |

### A 项核对

判定对应原始 [REVIEW.md](C:/dev/AGLIbot/docs/reviews/2026-11-15/REVIEW.md) 的故障场景。另列与六阶段直接相关、但未在提交标题中点名的项目。

| 项 | 判定 | 依据 |
|---|---|---|
| A01 | **部分** | `context.rs:61` 只读人格种子，旧的私人日记 → drift 自由文本 → 全局 L1 路径已消失。主人要求保留的有界成长和脱敏扩域尚无替代实现。 |
| A02 | **已关闭** | `scheduler.rs:290` 用固定提示向主人开场，决策 `reason` 只进审计。另一路任务标题泄漏见 R09。 |
| A03 | **部分** | `mongo.rs:360` 通过原子 `$inc` 分号并用唯一 `_id` 插入，两次创建不会覆盖 payload。后续 `save_review` 在 `:382` 仍仅凭 `_id` 整条替换，缺少原报告要求的版本校验。分号后插入失败只留下空号。 |
| A04 / A12 | **未关闭** | 进化执行和交接已撤下，过期基线发布与 PID 交接的原场景现在不可达。占位入口没有实现其发布验收条件。 |
| A05 | **部分** | `conversation.rs:249` 会重读状态和范围，常规 sighting、发送计数、管理编辑已是字段更新。删除仍能与重新创建交错，见 R11。 |
| A06 | **部分** | Ctrl+C、快速控制和 15 秒 watchdog 已独立于普通聊天；已注册任务可以被取消。准入竞态、存储故障和入口过载仍会破坏控制语义，见 R03、R04、R19。 |
| A07 | **已关闭（现后端）** | 生产入口只打开 Mongo，原 bot 与 Web 两个 JSONL 实例并发改写同一文件的路径已移除。 |
| A08 | **未关闭** | `migrate-bson` 处理旧 Mongo 包装记录，不能导入 JSONL 目录的权威 media 和 lexicon，见 R08。旧 Mongo 的 lexicon 本来就是原生字段，不需要包装转换。 |
| A09 | **已关闭** | `media.rs:555` 同时枚举数据库引用和磁盘文件，对超过保留期且无记录的媒体执行清理。已有测试覆盖旧孤儿、近期文件和受保护文件。 |
| A10 | **已关闭** | `qq.rs:293` 仅明确 `Rejected` 才退回普通分片。超时、断连及异步接受不会自动重发。 |
| A11 | **已关闭** | `qq.rs:265`、`:583`、`:593`、`:597` 将绝对期限覆盖入队、发送和回执，并清除过期 pending。 |
| A13 | **已关闭** | `work/mod.rs:243` 保留最终 revise/defer 为 Open，非 pass 结果带暂停原因；`:49` 将其映射为 Waiting。 |
| A14 | **已关闭** | `start-bot.ps1:14` 和 `start-stickers-web.ps1:29` 都在停止旧进程前检查候选存在性和 SHA-256。旧库切换失败是另一项发布问题，见 R07。 |
| A15 | **部分** | 新任务会复读补充并记录审查消费序号，完成前也会复查。旧任务升级时遗漏已经接受的 `followups.md`，见 R02。 |
| A16 | **未关闭** | `ops/deploy.ps1:24` 等路径仍固定为 `<runtime>/data`，Rust 仍支持自定义 `data_dir`。 |
| A17 | **已关闭** | `people.rs:176` 先复制并修改人物记录，存储成功后才替换缓存。保存失败不会让同进程重试误判事实已持久化。 |
| A18 | **部分** | `scheduler.rs:145` 自动摘要已先发送再标记；手动 `/digest` 仍是相反顺序，见 R17。 |
| A19 | **已关闭** | `scheduler.rs:118`、`:205` 在睡眠期间持续检查，按日最多尝试三次，失败后退避 30 分钟，摆脱了单次入睡边沿。 |
| A20 | **未关闭** | 原项是人物 `last_seen` 落盘节流。`spent_since` 已改用 Mongo 聚合，离线探针仍可复现原故障，见 R18。 |
| A21 | **部分** | `ingress.rs:254` 将每群 unseen 限为 80 条，解决条数无界增长。`chat.rs:298` 在 `speak=false` 后仍反复渲染同一窗口，没有已读游标或字符上限。 |
| A22 | **部分** | `reflect.rs:274` 现在查询 `consolidate + LocalOnly` 是否有合格模型，缺少时过滤 SelfOnly。日记路由已修复；原报告同时要求验收的周成长和人格探针已被删除。 |

### 安全与治理边界

- **下载器。** 在本地 HTTP 附件下载路径，`src/channels/urls.rs:58` 将实际连接固定到已验证地址，关闭自动重定向，`:81` 对每一跳重新解析和检查。静态审查未发现这条路径遗漏重定向校验。Telegram 的代理回归见 R14。
- **Codex runner。** `src/hands/codex.rs:187` 构造 `codex [--search] exec --sandbox workspace-write -C <任务子目录> [resume <UUID>] -m <model> --skip-git-repo-check --json -o <file> -`。编码任务关闭搜索，Codex 研究分支开启搜索。参数中没有 sandbox bypass，也没有显式 approval policy。子进程清空并重建环境，但允许继承 `CODEX_HOME` 和用户目录。本次没有启动真实委派，实际配置下的额外可写根、工具权限及审批行为**待确认**。R05 是 bot 父进程自身的写盘问题，独立于子进程沙箱。
- **写入授权。** 六个提交都修改了 `src/`。按 owner 提供的 Gemini 实现者身份及 `AGENTS.md:14` 的名单，Gemini 没有核心代码直写资格。`0fe9648` 明确删除了 owner-only 的 `src/evolution/gate.rs`；此范围没有修改 `config/` 或 `mind/self/boundaries.md`。阶段 5 另改动了 `src/ns/secrets.rs` 的执行器凭据规则。历史授权针对闸门的初始实现；本次删除及核心改写是否另获 owner 特批，**待确认**。

### 构建与测试

在指定 scratchpad 中从 `git archive 463363c` 导出的源码快照执行，工具链为 Rust/Cargo `1.95.0`。全部 Cargo 命令使用以下目标目录。

```powershell
$env:CARGO_TARGET_DIR='C:\Users\19260\AppData\Local\Temp\claude\a19ed9776312eff5\d8d3650f-2746-4757-aa5f-e2a321d3f519\scratchpad\codex-target'
```

| 工作目录 | 实际命令 | 结果 |
|---|---|---|
| `scratchpad\review-463363c` | `cargo check --locked --offline --all-targets` | 通过，退出码 0 |
| 同上 | `cargo build --locked --offline --bins` | `aglibot`、`stickers-web` 两个二进制均构建成功，0 失败 |
| 同上 | `cargo clippy --locked --offline --all-targets -- -D warnings` | 通过，0 条警告，0 错误 |
| 同上 | `cargo test --locked --offline --quiet` | **309 通过、0 失败、10 忽略**。库测试 304 通过，贴纸 Web 测试 5 通过 |
| `scratchpad\review-probes-463363c` | `cargo test --offline --test observed --quiet` | **4 通过、0 失败**。断言验证 R02、R03、R16、R18 的故障现象。见[探针源码](C:/Users/19260/AppData/Local/Temp/claude/a19ed9776312eff5/d8d3650f-2746-4757-aa5f-e2a321d3f519/scratchpad/review-probes-463363c/tests/observed.rs) |

