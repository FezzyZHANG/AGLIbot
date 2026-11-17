# Mind / social / privacy 审查（2026-11-15）

本报告只读核对生产实现、相关测试及 DESIGN/CONFIG；未修改 core/config，未调用付费模型，未读取环境凭据或线上数据。全套测试由主审代理执行，以下发现为静态调用链证据，新增故障注入和隐私回归测试仅列验收要求，尚未执行。

## 确定发现

### MS-01 [P1] 私人周反思产物被直接升级为全局人格提示，绕过 scope 与 Widen 审查

- 主位置：`C:/dev/AGLIbot/src/mind/growth.rs:117-137`；来源读取 `:169-179`；跨范围输出 `:70-79`。
- 来源→出口：`reflect::write_journal` 把当天跨会话 episodes 及可用的 self lessons 写入私人日记（`src/mind/reflect.rs:397-431`）→`growth::weekly` 读取近七篇日记，请模型生成 `voice_note` / interests→`apply` 只检查字数、单行、每周数量，未保留来源 scope、未调用 Widen→`growth::voice` 将原样文字附加到 voice→`context::build_l1` 加入所有会话系统提示（`src/mind/context.rs:59-75`）→夜间成功后 `agli::nightly` 立即 replace_l1（`src/agli.rs:2276-2277`）。之后任一群聊都能在模型输入中得到该文字。
- 正常触发场景：非主人 Alice 在私聊中谈到复诊；日记记下此事；周成长输出“聊天时体谅 Alice 正在接受某治疗”作为语气备注，或输出包含某个具体私人事件的兴趣变化。这不要求模型执行恶意指令；正常根据日记总结的文本即可越界。
- 影响：DESIGN 2b/2d 的检索前隐私门被派生状态绕开；即使模型最终不说出口，受限内容已进入群聊生成上下文。若原日记包含 local-only lesson，加载这些文字的远程 chat 模型还会绕过 processing policy。
- 修复：数值人格增量与文本人格产物分开处理；文本保留 scope/policy/provenance，只在相应 audience/policy 下加载。任何进入全局 L1 的派生文本都视为公开范围升级，先走 Widen 审查；或只允许不含原始自由文本的受限风格枚举。
- 验收：临时日记中写入私密 canary；stub 周成长正常返回含 canary 的 voice_note / interest。生成群聊 L1 时必须没有 canary，且未经审查不能公开升级。现有 growth 测试只验证备注被加入 voice，没有隐私验收。

### MS-02 [P1] solo 决策的自由文本 reason 被直接带入主人会话，丢失来源范围与处理限制

- 主位置：`C:/dev/AGLIbot/src/agli.rs:2369-2378`；来源 `:2329-2346`。
- 来源→出口：`commitments::due_soon` 按 due/status 选择所有人的承诺，不按主人 audience 过滤（`src/mind/commitments.rs:123-132`）→`Situation::include_commitments`/`decide` 在私人思考上下文中加入这些内容（`src/mind/decide.rs:62-64,118-129`）；有本地模型时还加入 self-only lessons（`src/agli.rs:2332-2342`）→模型的 `Decision.reason` 经 `validate` 直接成为 `Action::MessageOwner`（`src/mind/decide.rs:135`）→`reach_out` 把 reason 原文插入主人会话 `body`，交给 `converse_with_claim`。来源 scope/policy 在此丢失，主人会话随后只按自身 frame 选择模型。
- 正常触发场景：AGLI 答应私聊者 Alice 周五陪她去看病，承诺 scope 只有 Alice。到期时 solo 决策自然给出“想到 Alice 的病情，想向主人请教”的 reason；此 reason 未经任何分享判断就进入主人会话。另一场景是本地决策基于 self-only lesson 生成原因，之后 chat 使用远程模型。
- 影响：其他人的私聊记忆可进入主人上下文；local-only solo 材料可进入远程 chat。代码只限制能向谁发消息，未限制此跨边界自由文本能包含什么。
- 修复：对外行动规划使用目标听众可见的材料；solo 内部 reason 不作为聊天正文输入。行动只能返回枚举/引用，真正面向主人生成时重新按主人 audience 检索，并根据引用交集保留 processing policy。扩大范围时走 Widen。
- 验收：加入 scope=Persons[Alice] 的临近承诺，以及 self-only lesson；stub 决策正常返回引用 canary 的 message reason。捕获主人会话输入，必须没有 Alice-only/self-only canary；若必须使用派生内容，要求先有通过的范围升级审查且模型满足原 policy。

### MS-03 [P1] 并发审查使用 max(id)+1 分配编号，同编号保存会覆盖另一项

- 主位置：`C:/dev/AGLIbot/src/mind/judge.rs:115-127`；双后端 `src/store/jsonl.rs:285-298`、`src/store/mongo.rs:357-366`。
- 证据：`judge::file` 先 `next_review_id`，构造对象，再 `save_review`；两个动作没有同一个锁/事务。两种存储的 next 都读取当前 reviews 最大 id + 1；JSONL save 对已有同 id 替换，Mongo save 按 id upsert。
- 实际并发链：`src/main.rs:52` 默认多线程 Tokio；`src/agli.rs:1961-1970` 为 research job 克隆同一个 `Arc<dyn Store>` 后 `tokio::spawn`；作业在 `src/work/research.rs:726-734` 或 `:788` 调 `judge::file`。主循环同时可能在 `relay_request`（`src/agli.rs:1764`）、Widen、bot_flags 或 probes 写 review。max_jobs=1 时一个 job 与主循环就已可竞争；多 job 同时完成更明显。
- 触发与影响：两个调用都取到 N；第二个 save 替换第一个。待决项静默丢失，主人看到/批准的编号可能已关联另一条 payload，影响权限与审计可信度。
- 修复：新增原子的“分配并插入审查项”存储接口；Mongo 用原子计数器或不可冲突主键，JSONL 在同一存储写锁内分配和插入；已有项更新另外做版本检查。
- 验收：用 barrier 在两次 next 完成后放行 save，同时调用两个 judge::file，要求 ID 不同且两条 subject/payload 均保留。至少覆盖两个 job，以及 job+主循环路径。该发现与 store 子审重复，可主报告合并。

### MS-04 [P2] 事实先写内存缓存后落盘，保存失败后的同进程重试把未保存事实误判为已存在

- 主位置：`C:/dev/AGLIbot/src/social/people.rs:164-173`；调用与水位 `src/mind/reflect.rs:157-183`。
- 证据：`add_fact` 先向缓存中的 Person.facts push，再调用 save_person。保存失败时缓存未回滚，`extract_messages` 错误返回且未推进水位。同进程之后重试同批事实时，文本去重会直接返回 Ok，不再 save_person；其他步骤成功后水位可前进。因此磁盘从未保存过的事实被标为批次已处理，重启即丢失。此问题在不发生进程崩溃、单次正常自动重试时就可触发。
- 修复：先复制 Person、在副本中加事实，save_person 成功后再替换缓存；或失败时严格回滚当前变更，不能把失败的持久化当作已完成。
- 验收：Store wrapper 令首次 save_person 失败、下一次恢复；在同一个 People 实例重复 add_fact 或 extract_ns。最终重新打开 Store 后必须存在事实，水位不能在事实未保存时前进。
- 已知延期范围：反思其他结果与水位分步写入（`reflect.rs:128-183`），以及两个后端 append 不按领域 id 去重，确实造成崩溃/中断重试重复记忆；但 DESIGN 8 第420行明确把可恢复性、去重、一致性修复列为首版后续增强。本报告将这部分登记为已知缺口，不再作为新增设计违反项计数。未来可用批次checkpoint与幂等键修复。
- read-only 相关：read-only 预算在调用前持久化 used.messages / pending_since（`reflect.rs:270-272`）是保守记账；失败不退消息额度，同夜可能无额度重试。该策略与现有预算测试一致，不能仅凭失败不退额度另判 bug。

### MS-05 [P2] 每日摘要在确认可投递和发送成功之前标记已通知，失败后待决项永久消失

- 主位置：`C:/dev/AGLIbot/src/mind/judge.rs:161-178`；调用 `src/agli.rs:2188-2203`。
- 证据：`digest` 在构造文本时把每项 `notified=true` 保存。`maybe_digest` 先设 last_digest=今日，并在 tuple 的左侧调用 digest，随后才取得 owner_ctx。主人尚无私聊记录时 owner_ctx=None，仍已修改待决项。send 返回失败时也没有恢复 notified；以后 digest 只选择 `!notified`。
- 正常触发场景：启动后先有群友转告请求，在主人第一次私聊之前到了摘要时间；或者摘要当天 Telegram/QQ 确定发送失败。后续即便连接恢复，`/digest` 也不会重新包含这些 open review。
- 修复：先取得目标，再生成无副作用摘要；发送成功后确认 notified。需要避免“不确定是否已发送”时重复，可单独保存发送 attempt/outcome，明确失败项仍可重试。
- 验收：分别覆盖 owner_ctx=None、确定发送失败、成功发送、结果未知重启；前两种后续摘要必须保留 open review，成功只通知一次。

### MS-06 [P2] 夜间反思只挂在入睡边沿，任一可恢复失败使整夜后续工作不再尝试

- 主位置：`C:/dev/AGLIbot/src/agli.rs:2155-2162,2255-2266`。
- 证据：`state.tick` 已先把 energy.asleep=true 并发出一次 FellAsleep（`src/mind/state.rs:214-217`）；nightly 由该事件触发。extract_all、read_only_nightly、knowledge::consolidate 或 write_journal 失败都 `?` 返回；虽未写 last_reflect，下次 tick 已处于 asleep，不会再发 FellAsleep，也没有“当前睡眠且本日未完成”补跑路径。
- 影响：一次短暂模型/磁盘/Mongo 故障，或一个不完整 Done 研究任务导致 knowledge 错误，会跳过当夜日记、关系更新、卡片导出、周成长、探针与归档。不是单独跳过坏任务。
- 修复：以持久化的日期/阶段完成状态驱动夜间任务；失败后有限退避重试，每个阶段可单独继续，坏任务不能阻断所有下游阶段。
- 验收：入睡时令 journal 或 knowledge 第一次失败，后续恢复；保持 asleep 再 tick，要求当晚剩余阶段继续完成，已成功阶段不重复付费。

### MS-07 [P2] last_seen 的十分钟节流使用不断刷新的缓存时间，活跃用户可无限期不落盘

- 主位置：`C:/dev/AGLIbot/src/social/people.rs:65-92`。
- 证据：changed 以 now - p.last_seen >=10分钟判断，随后每条消息无论是否落盘都令 p.last_seen=now。同一人在相同群/私聊、名称/平台/tier不变、每次间隔<10分钟时，比较值永远不够10分钟，即使已经活跃几个小时。
- 影响：存储中的 last_seen 长期停在首条消息，重启与人物卡的最近活动时间错误。reciprocity::nightly 从 store.persons 加载旧对象并 people.save，又会把过时 last_seen 写回内存。
- 修复：另外维护 last_persisted_seen，或按固定时间桶/脏记录周期落盘；仅在存储成功后更新落盘基准。
- 验收：相同用户 t0 起每分钟发言30分钟，中间不改变元数据；持久化 last_seen 应至少每10分钟更新，重载后接近最新消息。

## 设计 / 文档 / 实现对齐矩阵

| 项目 | 设计 / 文档 | 现实现与判断 |
|---|---|---|
| 普通召回 scope | DESIGN 2b，先过滤再构建上下文 | `privacy::recall`、semantic recall 及 recap 都先按 audience 过滤；group membership 进私聊的单向扩展已有测试。没有发现这个基本过滤器自身绕过。MS-01/02 是绕过该入口的派生文本问题。 |
| self/local-only | DESIGN 2b/5b | `Recall::policy`、commitments extraction/decide、reciprocity、archive 能识别 SelfOnly；conversation appraisal 能合并 Providers 交集。`write_journal` 只根据 has_local/lessons 选 policy，没有检查 episodes.scope。日记/自传/overlay 又不存 provenance，切换到无本地模型后可能将历史 local-only 派生内容交给 Any；需要跟 MS-01/02 一起补齐数据血缘。 |
| 手工 namespace policy / provider 细分 | DESIGN 2b 说可手工覆盖 | domain Episode/Fact/Lesson 无独立 processing_policy，也没有 namespace override 加载器。当前大部分调用仅从 SelfOnly 推 LocalOnly；Providers 类型存在但不是完整的数据级策略。作为尚未落实设计能力单列，不宣称当前配置已生效。 |
| Widen | DESIGN 2d/5d | lessons widening 确实调用 judge，Pass 另存 Public 副本；非 Pass 保留 SelfOnly / Owner review。review 不可用会转 Owner；read-only SpendLimit 也传到 review，预算不足不会强行发起付费调用。MS-01/02 的非 lesson 派生文本未复用这道门。 |
| Widen 的 processing policy | DESIGN 2b hard constraint | `judge::review_with_limit` 一律 Any（`src/mind/judge.rs:95`），不接受 caller policy。extract 可能因 self-scoped来源/既有承诺被选 LocalOnly，但其 shareable lesson 随后被送 Any reviewer（`reflect.rs:119-125,169-193`）。支持 self-scoped输入时这是策略弱化；普通群/私聊默认原始 policy 本来 Any，不能据此声称每次远程 review 都违规。需让 review 接收来源 policy 并取交集，或明文限定授权的 review 例外。 |
| read-only groups | DESIGN 6，加入群默认只读 | on_message 不回、不建工具任务；独立夜间采样，受金额与消息数限制。采样后水位推进到完整窗口结尾，会故意略过未采样消息，代码和现有测试一致，不当作丢消息 bug。 |
| read-only 预算 | CONFIG 224-226，准入金额可被最后一调用越过 | 准入上限而非硬预留，extract、Widen、archive共享记账；代码与文档一致。journal / reciprocity 等对采样产物的再次思考不计入此专用预算；文档应明确它不是“只读群所有后续影响的全成本”。 |
| ignored groups | CONFIG 211“不反思” | extract/archive 排除 ignored；journal 全 episodes、reciprocity 全 originals 并未按 social 配置排除已存的 ignored 群记忆，普通 semantic recall 也可向曾经成员召回旧记忆。需要明确“只停止新增群处理”还是“彻底排除历史记忆”，当前不能由配置表推导为后者。 |
| 人物卡 | DESIGN 2a 注释，导出 data_dir/people | exporter 用 owner audience 过滤事实/经历，排除 owner/agent，限制数量并稳定写文件，符合当前说明。人物 trust/intimacy 等无 scope 的社会元数据是另一类设计选择，不能由“Facts已过滤”推断所有元数据具相同粒度。 |
| 状态文件 | DESIGN 树还列 mind/state 快变量 | 实际状态写 data/state/state.json / flow.json，mind/state 为 seed；flow/state 模块注释已说明，设计树应标明种子与运行态区别。 |
| 群表情与贴纸 | DESIGN 1c 旧状态写“尚未接入” | expression/media/vision 以及 L1 marker 已集成；该状态注释已过期。sticker-store 研究文档应作为现行为引用，不再把旧 memes 研究结论当最新状态。 |
| 研究知识层 | DESIGN 2a/8 | knowledge 从 Done research 构建卡片、来源信誉与争议立场文件，使用 checkpoint；已核对其输入路径规范化、claim index覆盖与引用验证标记。尚未在聊天 L1/L2 看到 knowledge/narrative 文件被召回，这与“已构建知识体系”和“已在聊天中使用”应分开记录。 |
| 成长上限与人格探针 | DESIGN 6 / CONFIG memory | 数值单次±0.02、累计±0.15、每周状态门与探针低分送 owner 均落实且有测试。文本变化仅长度/数量校验；人格一致性探针不能替代隐私 Widen 审查。 |
| 崩溃后反思重试 / 幂等 | DESIGN 8:420 明确后续增强 | 当前 append 结果与提炼水位分别提交，崩溃/中断可能重复写记忆与模型调用。这是已知延期，单独列出，不当成新增设计违反；MS-04 的同进程失败缓存污染仍需修复。 |

## 覆盖与验证边界

已逐一阅读以下生产文件：

- `src/mind/{context,decide,speak,state,flow,flow_metrics,reflect,growth,probes,reciprocity,commitments,watchlist,knowledge,judge,expression,mod}.rs`。
- `src/social/{privacy,people,groups,identity,floor,bot_flags,cards,lexicon,skill_lines,mod}.rs`。
- 阅读了 social privacy semantic / groups / cards、mind commitments / growth / probes / reciprocity 的独立测试，以及上述生产文件内主要测试；交叉核对 `agli.rs` frame/think/reach_out/nightly/digest/start_job、Store trait/JSONL/Mongo 的相关调用链。
- 读取 DESIGN 全部相关章节，CONFIG 的 memory、social、policy 与热加载说明。

未承接 media/vision/embedding/models/router/llm 的完整审查，也未承接 hands/evolution/ops；由其他子审覆盖。未做真实 QQ/Telegram 投递、Mongo 压力/故障、模型隐私行为或运行时数据审计。这里的建议验收以临时数据和 stub 模型验证强制边界，不以模型“通常会保密”作为通过标准。

## 根审查候选的独立复核

### ROOT-C1 [P2，确认] 潜水群的 unseen 没有数量、时间或字符上限，triage 反复收到累计全历史

- 精确链：白名单可发言群每条输入在 `src/agli.rs:1109-1110` 向 `HashMap<String, Vec<UnseenLine>>` push；`consider_group` 在 `:1212` clone 全部行，`:1216` render 全量，`:1223` 发 triage；`speak=false` 直接返回（`:1225-1228`），不移除已判断消息。只有真正进入 `converse_group` 的 `:1239` 才 remove。`render_unseen` 的 `:606-614` 对每一行 join，不截断。floor.considered 仅复位消息计数，不清理 unseen。
- 正常触发：一个活跃白名单群连续数小时未叫他、每轮 triage 合理选择不插话；或心流忙碌期持续积压。内存随消息数增长，后续 triage 每隔 check_every 再提交更长的历史，累计输入量可呈二次增长；达到模型上下文上限后失败，旧记录仍保留。后来被@还会把全部积压送入聊天，可能连直接回应也失败。
- 设计判断：DESIGN 4c 的 pending 是暂存、退出时压缩，不是无界历史授权；未找到接受此资源无上限的延期说明。这是确定后端资源/可用性缺陷。
- 修复与验收：为 unseen 设置每群条数/时间/字符预算；已判断“不说”的行推进阅读游标，必要背景保留固定窗口，原文仍在 Store。用 stub 连续返回 speak=false，灌入数千条消息后，内存行数、每次triage字符数有固定上限，最新@仍被保留并正常回答。

### ROOT-C2 [P1，控制契约冲突确认；串行架构本身是已记录限制] 慢模型调用阻断控制指令、Ctrl+C 与任务级 watchdog

- 精确链：`src/main.rs:158-167` 的 select 选中 work 后，在分支内部 `agli.handle(work).await`；该 await 未被外层 select 的 Ctrl+C/config_poll 再包围。`Agli::handle` 在 `src/agli.rs:734-737` 串行 await 消息/tick处理；普通 triage/chat 可等待模型。HTTP client 总请求期限为600秒（`src/mind/llm.rs:283-285`），模型升级/重试、夜间多次调用还能串接更长等待。与此同时新 /stop、/freeze 和 Tick 只能排队。
- watchdog 只由 `on_tick` 首行调用（`src/agli.rs:2153-2154`），最终停止任务依赖 `:2054-2069`；所以一个仍在后台执行的 hands 作业可能在主循环等模型时超过任务级 stall/总时长预算，且主人 kill switch 也无法及时到达。子进程自身的每次调用 timeout 仍独立生效，不能说所有超时都失效。
- 设计判断必须保留的限定：DESIGN 10a 第456行已经写明“聊天模型调用串行占住主循环”，10c 第475行又把全双工 start/stop 工具列为暂缓，故“需要全双工”本身不是新发现。但是 DESIGN 9 第437行同时要求“/freeze 立即生效”，CONFIG 宣称 watchdog 按 tick 检查；当前串行 await 实际让这条控制/停机契约不能成立。建议主报告按安全控制的优先通道缺失报P1，同时写出已知架构限制，不能描述成设计从未记录阻塞。
- 最小修复：让主人控制/取消、退出信号与任务期限监测独立于长时间生成；普通会话仍可串行。可对生成任务使用可取消句柄/事件返回状态，而不是无限扩大主状态的并发写入。
- 验收：临时模型服务接收请求后悬挂，在请求尚未完成时注入 /freeze、/stop 和退出信号，并令一个后台任务跨 watchdog 到期点。要求控制在明确的小期限内响应、任务被终止，而不必等HTTP600秒；同时证明子进程清理与状态审计仍完整。
