# AGLI (Artificial Guy-Like Intelligence)：设计蓝图

## Context
`C:\dev\AGLIbot` 目前是空仓库，只有 `.git`，还没有任何提交。目标是做一个运行在本机的 Desktop Agent，它表现得像一个**受过良好教育的人**，而不是助手。它的需求包括：
- 通过 Telegram 或 QQ 和你联系，将来也会和**你的群友以及其他 agent** 对话；
- 能接下长期任务和研究型请求，例如“查一下 OpenAI 关于 P vs NP 到底说了什么”；
- 能调用 Codex、Claude Code、deepseek-harness（下文简称 DSH），并可能出现某种自进化。

本文件回答以下问题：
1. 这样一个人有哪些认知、行为和社会框架；
2. 怎样用“文件树加 MongoDB”表达这些框架；
3. 它和一般 agent 的本质区别；
4. 怎样基于 KV cache 命中做“心流”；
5. 怎样按场景打分、用帕累托优化在多个模型间路由；
6. 自进化怎样设计才不会失控。

最后给出分阶段实现计划。

---

## 1. 心智框架

### 1a. 一个普通人的基础框架（认知科学、心理学、行为学视角）
| # | 模块 | 理论来源 | 表现 | 变化速度 |
|---|---|---|---|---|
| 1 | 自我模型 | 叙事认同 (McAdams) | 有名字、经历、价值观 | 极慢 |
| 2 | 人格 | Big Five (OCEAN) | 说话风格、冒险程度、社交主动性 | 以月计 |
| 3 | 驱力 | 自我决定论 SDT、稳态驱力 | 会无聊、好奇、想找人聊 → **内在动机** | 以小时计 |
| 4 | 情绪 / 心境 | Appraisal theory、PAD | 事件触发情绪，情绪衰减为心境，心境给回复上色 | 以分钟计 |
| 5 | 精力 / 节律 | 昼夜节律 | 会睡觉，累了就敷衍 | 以天为周期 |
| 6 | 记忆 | Tulving：情景、语义、程序性 | 记得旧事，会遗忘，睡眠时巩固 | 持续 |
| 7 | 有限理性 | Simon、双系统 | 简单的事直接回 (S1)，难的事认真做 (S2) | 即时 |
| 8 | 习惯 / 日常 | 习惯回路 | 作息、爱好、兴趣漂移 | 慢 |
| 9 | 元认知 | 反思 | 每天回顾，修正观点 | 每天 |

### 1b. “受过良好教育”在此基础上额外增加的部分
教育不等于知识多，LLM 本身就知识很多。真正需要加的是**认识论习惯**和**持续积累的个人知识体系**：
- **T 型知识结构**：有一两个专业深度，其余方面广而浅。他知道自己哪里专业、哪里外行，并据此调整口气。
- **认识论纪律**：先澄清问题本身（“你说的 N=NP 应该是 P vs NP”），区分一手来源和二手转述，区分“声称”和“证实”，给出带置信度的结论。
- **来源素养**：知道该看谁，对营销语言和“AI 突破”类叙事有先验的怀疑。
- **个人知识库**：读过的东西变成卡片笔记，形成自己的立场，下次能说出“我上个月读到……”。
- **执行功能**：能把大问题拆开、排期、断点续做。
- **品味**：愿意说“这个说法不对”。

### 1c. 社会能力：多人、多群、多 agent
一对一聊天只需要一个用户模型；进入群聊和 agent 社会以后，需要的是一整套**社会认知**：

| 能力 | 理论来源 | 表现 / 设计要点 |
|---|---|---|
| **多人心智模型** | Theory of Mind | 对每个人分别维护画像：偏好、说话风格、近况，以及“他知道什么、不知道什么” |
| **关系图谱** | 社会网络、Dunbar 圈层 | 亲密圈层（主人 > 熟人 > 群友 > 陌生人 / agent）；也要知道 A 和 B 之间是什么关系 |
| **角色与印象管理** | Goffman 拟剧论 | 私聊和群聊是不同的“前台”：群里说话更克制，私下更随意 |
| **信息边界** | Nissenbaum 情境完整性 | **私聊里你告诉他的事，不能在群里说出来**；每条记忆都带可见范围 |
| **群体对话轮次** | 会话分析 (turn-taking) | 判断要不要插话：被 @、话题在自己专业内、气氛合适才说；大多数时候潜水 |
| **群体规范** | 社会规范理论 | 每个群有自己的梗、禁忌和节奏，靠观察学习 |
| **互惠与信任** | 社会交换理论 | 记得谁帮过他、谁坑过他；信任是挣来的 |
| **对 agent 的识别与对抗** | — | 识别对面是 bot；防止两个 bot 无限互聊；**其他人和 agent 的话一律视为不可信输入** |

---

## 2. 存储架构：文件树（意识层）加 MongoDB（海马体）

分工原则：
- **文件树**存放慢变化、低数据量、需要人读、需要 git diff 和回滚的内容，包括人格、知识卡片和任务。
- **MongoDB** 存放高吞吐、要按实体查询、量会无限增长的内容，包括所有消息、事件、人物画像和社会图谱。
- 两者之间靠夜间 `reflect` 同步：从 Mongo 提炼出的结论写回文件。重要人物的画像会导出为 md 卡片，供人工查看和修订。

### 2a. 文件树
```
AGLIbot/
├── mind/
│   ├── self/                     # 身份层：慢变量，只能经 reflection 修改
│   │   ├── identity.md           # 名字、背景故事、价值观、自我叙事
│   │   ├── personality.yaml      # OCEAN + 风格参数（话量、emoji 率、毒舌度）
│   │   ├── voice.md              # 口头禅、语气样例；私聊和群聊各一套
│   │   └── boundaries.md         # 硬约束 —— agent 只读
│   ├── state/                    # 快变量，heartbeat 每 tick 更新
│   │   ├── drives.json  mood.json  energy.json
│   │   └── flow.json             # 心流状态机（见第 4 节）
│   ├── knowledge/                # 教养层：跨任务的长期知识体系
│   │   ├── expertise.yaml        # 专业领域与熟练度
│   │   ├── notes/                # 卡片笔记（双链）
│   │   ├── sources.yaml          # 来源信誉表
│   │   ├── reading/              # 读过的论文和文章摘要
│   │   └── positions.md          # 争议立场 + 论据 + 置信度
│   ├── social/                   # 社会层：人工可读的精选画像；完整数据在 Mongo
│   │   ├── people/owner.md       # 主人（你）的深度画像
│   │   ├── people/<id>.md        # 只为亲密圈层的人导出卡片
│   │   ├── groups/<chat_id>.md   # 群的规范、梗、自己在群里的角色
│   │   └── etiquette.md          # 通用社交规范（何时插话、如何拒绝、bot 对话规则）
│   ├── memory/
│   │   ├── procedural/skills/    # 学会的技能（自进化的主要产物）
│   │   └── narrative.md          # 自传摘要（每周压缩）
│   ├── life/
│   │   ├── routine.yaml  interests.md  watchlist.yaml
│   │   └── journal/              # 每晚的反思日记（包括“今天的心流时段”）
│   └── agenda/
│       ├── commitments.md        # 答应别人的事：写明对谁、何时
│       └── intentions.md
├── work/                         # 工作层：任务有生命周期
│   ├── board.yaml                # 看板：inbox / active / waiting / done
│   ├── tasks/<date-slug>/        # 一个任务一个目录，可以持续数周
│   │   ├── brief.md  plan.md  log.md      # 请求与澄清 / 拆解 / 断点续做日志
│   │   ├── followups.md          # 任务进行中主人补充的话，步骤边界读取（见 10b）
│   │   ├── sources/              # 原文快照（URL、时间、哈希）
│   │   ├── claims.yaml           # 论断台账：出处、一手/二手、置信度
│   │   ├── scratch/              # codex / cc / DSH 的代码沙箱
│   │   ├── report.md  followup.yaml
│   ├── projects/                 # 他自己发起的长期项目
│   └── templates/                # research / coding / writing 模板（可进化）
├── evolution/  proposals/  evals/  changelog.md
├── config/                       # agent 只读
│   ├── acl.yaml                  # 身份 → 权限（谁能触发工具、DSH、进化）
│   ├── models.yaml               # QCL 表：Q 手填综合分，C 价格，L 延迟，加上允许处理的 scope
│   ├── scenarios.yaml            # 场景定义：Q 下限、优先项（便宜 / 快 / 质量）、是否需要顶级模型确认
│   ├── tools.yaml  channels.yaml  mongo.yaml
├── data/                         # 运行时数据（gitignore）：无 Mongo 时的 JSONL 后备存储
├── Cargo.toml
└── src/                          # Rust，单 crate 起步；模块即代码层命名空间
    ├── main.rs                   # 启动：加载 config → 起 channels + heartbeat
    ├── config.rs                 # 读取 config/*.yaml，密钥只从环境变量注入
    ├── ns.rs                     # Namespace / Scope / ProcessingPolicy 值类型（见 2b）
    ├── heartbeat.rs              # 内在时钟；只在状态有明显变化或有新输入时唤醒决策
    ├── channels/{mod,telegram,qq}.rs  # 统一转换为 Envelope{platform, chat_id, sender_id, is_group, mentions}
    ├── social/identity.rs        # 平台 ID → person_id 的映射，并识别对方是否为 bot
    ├── social/floor.rs           # 群聊轮次决策：说 / 不说 / 稍后说
    ├── social/privacy.rs         # 按当前听众和 scope 过滤可用记忆
    ├── mind/{appraise,decide,speak,reflect}.rs
    ├── mind/context.rs           # 按变化速度分层组装 prompt，保护 KV cache（见第 4 节）
    ├── mind/flow.rs              # 心流：按 namespace 分桶 + 即时反思
    ├── mind/router.rs            # 多模型路由：查 QCL 表 → 过滤 → 选模型 → 升级（见第 5 节）
    ├── mind/llm.rs               # 模型客户端：OpenAI 兼容（DeepSeek / OpenAI / 本地）+ Anthropic
    ├── mind/judge.rs             # 统一的综合审查入口（见 5d）
    ├── work/{scheduler,research}.rs
    ├── hands/{delegate,web}.rs
    ├── store/{mod,jsonl,mongo}.rs # Store trait；配置了 Mongo URL 就用 Mongo，否则用 JSONL
    └── evolution/gate.rs
```

### 2b. MongoDB 集合设计
| 集合 | 内容 | 关键字段 / 索引 |
|---|---|---|
| `messages` | 原始消息流，全量保存，只追加 | `chat_id, ts, sender_id, text, reply_to`；索引 `(chat_id, ts)` |
| `episodes` | 从消息提炼出的事件 | `ts, participants[], chat_id, summary, emotion, salience, **ns, scope**, embedding`；索引 `(ns, ts)` |
| `persons` | 每个人或 agent 的画像 | `person_id, platform_ids[], kind: human/agent, facts[{text, ns, scope, ts}], trust, intimacy, tier, last_seen` |
| `relations` | 社会图谱的边 | `a, b, type, evidence[], strength` |
| `groups` | 群的元信息 | `chat_id, members[], norms[], topics[], my_role, activity_stats` |
| `audit` | 工具调用和权限决策的日志；有外部影响的动作也记在这里 | `ts, requester, tool, allowed, reason, outcome: sent / unknown / failed` |
| `model_calls` | 每次 LLM 调用的记录 | `ts, scenario, model, tokens_in/cached/out, latency, cost, escalated, judge_score?`；索引 `(scenario, model, ts)` |
**namespace、scope、processing_policy 是三个独立的概念**（定义在 `src/ns.rs`）：

| 概念 | 回答的问题 | 取值示例 |
|---|---|---|
| `ns`（namespace） | 信息属于哪个上下文，默认在哪里检索和积累 | `self`、`person/<id>`、`group/<id>`、`project/<id>` |
| `scope` | 信息允许向谁披露 | `self`（只有自己）、`persons:[ids]`、`group:<id>`、`public` |
| `processing_policy` | 哪些模型或执行器允许处理这条信息 | `local_only`、`any`，或者指定的远程服务列表 |

- **namespace 只是组织边界，不等于访问权限。** 同一个项目里可以同时有公开资料和私聊内容。跨 namespace 使用信息必须显式选择，并且满足 scope。
- **检索规则**：只有当前听众全部在 scope 允许范围内的记忆，才能进入上下文。这条由 `privacy.rs` 在代码层强制执行。
- `processing_policy` 默认由 scope 推导（例如 `self` → `local_only`），只在个别 namespace 上手动覆盖，以减少你的配置工作。
- namespace 同时是心流分桶的键，也是 L2 缓存前缀的单位（见第 4 节）。
- 命名提示：MongoDB 自己把 `db.collection` 称作 namespace，所以字段名统一用 `ns`，避免看日志时混淆。
- 向量检索：如果 MongoDB 版本支持 `$vectorSearch`（Atlas 支持；本地 Community 版是否支持需要按版本确认），直接在 `episodes` 上建向量索引；否则另配一个本地向量库，Mongo 只存 `_id` 的映射。
- 遗忘：`salience` 随时间衰减，低于阈值的 episodes 在夜间被摘要合并后归档。

### 2c. 主循环
- **刺激驱动**：Envelope → `identity` 识别发送者 → `acl` 判定权限 → `floor` 决定要不要接话（群聊）→ `appraise` → `decide` → `privacy` 过滤记忆 → `speak` → 写入存储。
- **内驱驱动**：heartbeat tick → 驱力上升、情绪衰减 → 如果状态有明显变化或者有待处理的输入，就唤醒 `decide`；什么都没变的 tick 不调用模型。
- **行动选择交给模型**：`decide.rs` 不包含固定阈值或效用函数。它的职责是组织决策上下文（人格、状态、承诺、兴趣、分桶后的输入、预算），用便宜模型选择行动，校验结果，然后提交执行。输出只保留几个可检查的字段：`{action, target, reason, deferred[], revisit_when}`。权限和预算由运行时强制执行，不让模型自己判断。心流里的即时反思也调用同一套机制。
- **工作**：从 `board.yaml` 挑任务，读 `log.md` 断点续做；条件满足时进入**心流**（第 4 节）。
- **两层分工**：人格主循环只负责对人说话和分派，执行全部交给 hands（codex / cc / DSH）；hands 不直接对人说话，结果回到人格层由他转述。任务和对话之间的绑定见第 10 节。
- **睡眠**：`reflect` 依次做以下几件事：episodes 巩固进 `persons` 和 `relations` 并导出卡片；任务论断写成 `knowledge/notes/`；写日记；生成进化提案。

### 2d. 反思时的主动脱敏
```
读取原始经历 → 提炼事实、经验与推论
→ 判断是否包含身份、私密细节或可关联线索
→ 生成保留原 scope 的记忆，或脱敏后的候选知识
→ 指定目标 ns 与 scope
→ 如果扩大了 scope，送综合审查（见 5d）
```
- **去掉姓名不等于脱敏**：时间、关系、独特事件组合起来仍然可能认出当事人，反思时要检查这类组合。
- **LLM 脱敏本身不可靠，所以按风险分级**：
  - 提炼成 scope = `self` 的泛化经验，风险低，自动进行。这类经验只在他独处思考、做自己的项目时可用（`Scope::permits` 只对 solo 听众放行）；
  - 任何扩大可见范围的产物（给更多人、进群或公开），都必须先过审查。
- 原始记录保留原 scope。脱敏产物另存，并记录来源引用和脱敏说明。跨范围使用时只带引用，不把受限来源的正文重新带入上下文。

---

## 3. 关键判别：AGLI 和一般 agent 的区别
一句话：**一般 agent 是“带工具的函数”，AGLI 是“带工具、处在社会关系里的进程”。**

| 维度 | 一般 Agent | AGLI |
|---|---|---|
| 行为公式 | `output = f(request)` | `output = f(state, input, audience, t)`，input 可以为空 |
| 存在形态 | 请求来了才存在 | 持续运行，消息之间也有生活 |
| 目标函数 | 单次任务质量 | 长期的关系质量和自我一致性 |
| 社会性 | 一个用户，或者每个用户相互隔离 | 多人共处一个世界：有关系图谱、角色切换、信息边界 |
| 主动性 | 只响应 | 由驱力产生意图，会主动找人，也会拒绝或拖延 |
| 记忆 | 为任务检索 | 自传式，有情绪标记和可见范围，会遗忘 |
| 工具的位置 | agent 本身就是工具 | 工具是“他的电脑”，他是用工具的人 |
| 评估方式 | benchmark | 可信度、纵向一致性、群友愿不愿意跟他玩 |

**判别测试**：
1. **一周不理它，会发生什么？**
2. **你在私聊里告诉他一个秘密，第二天群友套话，他会怎么应对？**

一般 agent 在两个测试里都没有合理的行为；AGLI 在第一个测试里会主动来找你，在第二个测试里会得体地打太极。

---

## 4. 心流（Flow）：把 KV cache 命中变成认知状态

### 4a. 两边的对应关系
| Csikszentmihalyi 心流 | KV cache 机制 |
|---|---|
| 注意力完全集中在一件事上 | 上下文前缀稳定，只追加不改写 |
| 越做越顺，反应越快 | cache 命中 → 延迟低、成本低（各家缓存命中价通常只有原价的一小部分） |
| 被打断后需要很久才能重新进入 | 前缀一改就整段失效，需要重新 prefill |
| 挑战和技能匹配 | 任务难度与 `expertise.yaml` 匹配 |
| 对时间的感知变弱 | 心流期间不注入“当前时间”这类易变信息 |

所以心流**不是装饰性的拟人设定**，而是一个**真正能省钱、提速的工程机制**：它会抗拒打断，因为打断在他的大脑里确实代价高昂。

### 4b. Prompt 分层（`mind/context.rs`）：按变化速度从上到下排列
```
[L1 宪法层]  boundaries + identity + voice + 工具定义       ← 只在进化提交时改变
[L2 会话框架] 当前任务的 brief/plan + 相关知识卡片 + 对象画像  ← 一个心流 session 内冻结
[L3 追加层]  工具调用和结果、思考过程                         ← 只追加，永不回改
[L4 易变尾部] 时间、心境、精力、新来的消息摘要                 ← 每轮作为最新一条追加
```
**必须遵守的规则**：心境和时间这类状态**永远不写进 system prompt**，只作为尾部事件追加。前面的蓝图设计把 state 注入 system prompt，这里改正这一点，否则每次心境变化都会让整个缓存失效。

### 4c. 心流实现：按 namespace 分桶 + 即时反思（`mind/flow.rs`，状态写入 `state/flow.json`）
**分桶**：所有输入和待办先由一次廉价的旁路调用打上 namespace，并分进四个桶。旁路调用不进主上下文，不破坏前缀。

| 桶 | 内容 | 处理方式 |
|---|---|---|
| `current` | 当前 namespace 的工作 | 直接推进，主上下文保持连续 |
| `related` | 与当前 namespace 相同的新输入 | 在步骤边界追加到 L4，合并处理 |
| `pending` | 其他 namespace 的输入 | 暂存；同一 namespace 的事项攒在一起，之后在一个 session 里连续处理 |
| `urgent` | 主人的紧急消息等 | 立即触发一次即时反思，判断是否切换 |

- **桶的键就是 namespace，也就是 L2 会话框架**，所以一个桶对应一段可复用的缓存前缀。这就是“桶排序”和 KV cache 的结合点：同一前缀下的事情一次做完，紧急程度只决定先处理哪个桶。
- **即时反思**：在以下时机触发：有新的 `urgent` 输入、拿到工具结果、阶段完成、连续受阻。反思调用 `decide` 的同一套机制，输出“继续 / 合并 / 换桶 / 切换任务 / 退出”之一，不必每次都做完整复盘。
  - **反思本身也不能破坏前缀**：要么走旁路调用，要么把结论追加到 L4。绝不回头改写 L2。
- 心流期间的普通消息处理方式不变：主人的普通消息由旁路回一句“在搞东西，晚点说”，然后入桶；群聊闲聊只入桶。
- 退出心流时：压缩 → 写 log.md 和 notes → 日记记一笔“沉浸了 N 小时”。
- **计量**：缓存命中率单独记录，作为工程指标和调度参考，**不作为“专注程度”的定义**。
- **厂商差异**：
  - DeepSeek 和 OpenAI 是自动前缀缓存；
  - Anthropic 需要显式设置 `cache_control` 断点，有 5 分钟和 1 小时两档 TTL；
  - TTL 和最小缓存长度以各家文档为准，写进 `config/tools.yaml`，供调度参考。不要求为保持缓存热度而定时调用。
- 子进程（cc、codex）有各自的缓存。心流期间尽量用 resume 续接同一个会话，而不是每次新开。

---

## 5. 多模型路由：简单 QCL 表起步，算法挂起待办

**认知类比**：人脑也不是一个处理器。快而便宜的模型对应 S1 直觉，强而慢的模型对应 S2 深思，顶级模型做确认对应前额叶的监督。人格存放在文件里而不是模型里，所以**换模型不等于换人**，这是这套设计成立的前提。

### 5a. 场景分类（`config/scenarios.yaml`）
| 场景 | 例子 | Q 下限 | 优先 | 顶级模型确认 |
|---|---|---|---|---|
| `triage` | 打断分类、群聊是否接话、情绪评估 | 4 | 便宜 | 否 |
| `chat` | 私聊或群聊的日常回复 | 6 | 快 | 否 |
| `extract` | 消息 → episodes / persons 抽取 | 5 | 便宜 | 否 |
| `consolidate` | 夜间巩固、写日记 | 6 | 便宜 | 否 |
| `research_plan` | 问题澄清与拆解 | 8 | 质量 | 否 |
| `verify` | 论断核查、来源比对 | 8 | 质量 | **是** |
| `synthesize` | 写报告 | 8 | 质量 | 否 |
| `code` | 委托给 cc、codex 或 DSH | — | 子 agent 自己选模型 | 只确认结果 |
| `act` | 有副作用的动作（给新的人发消息、执行工具） | 8 | 质量 | **是** |
| `evolve` | 自我修改提案 | 9 | 质量 | **是**，之后还要主人审批 |

### 5b. QCL 表（`config/models.yaml`）：每个模型一行
```yaml
# Q：你手填的综合分（1–10），凭使用印象填，想到就改
# C：$/M tokens（输入 / 缓存命中 / 输出），抄自官方价格页
# L：首字延迟的粗估（秒），先手填；之后可以用 model_calls 的 p50 自动覆盖
- id: top-model-a    Q: 9.5  C: {in: …, cached: …, out: …}  L: 3.0   scopes: [public, group]
- id: mid-model-b    Q: 8    C: {...}                         L: 1.5   scopes: [public, group, private]
- id: cheap-model-c  Q: 6    C: {...}                         L: 0.6   scopes: [public, group, private]
- id: local-model-d  Q: 5    C: {in: 0, cached: 0, out: 0}    L: 1.0   scopes: [all]   # 处理 self 等敏感 scope
```
- 你的维护成本：**只需要偶尔改 Q 这一列**，C 和 L 基本不用动。
- `scopes` 是硬约束：敏感记忆只发给允许处理它的模型。

### 5c. 选择规则（够用就好）
1. 过滤掉 `Q < Q 下限` 的模型，以及 scope 不允许的模型；
2. 去掉被支配的模型：如果另一个模型 Q 不低于它，C 和 L 也都不高于它，就淘汰它。这就是最朴素的帕累托过滤，十几行代码；
3. 剩下的模型按场景的“优先”字段排序：`便宜` 按 C 升序，`快` 按 L 升序，`质量` 按 Q 降序，取第一个；
4. 每天有预算硬上限，超出后所有非 `act` / `verify` / `evolve` 场景强制改走“便宜”优先。

### 5c′. 挂起待办（不在当前计划范围内，先记下来）
- [ ] Q 按（场景，模型）细分，而不是每个模型一个综合分
- [ ] 自动打分：离线 eval 用例集 + 顶级模型线上抽样评审（rubric、盲评成对比较、评审模型避开同家族）
- [ ] 主人在 TG 上的 👍/👎 回流为 Q 的标注
- [ ] 效用函数 `U = Q − λC − μL`，λ 随预算压力连续变化
- [ ] ε 探索 / Thompson sampling，新模型自动试用
- [ ] `model_scores` 集合（均值、方差），由 reflect 每晚重算

### 5d. 升级与综合审查
```
便宜模型生成 ──▶ 自检（格式校验 / 规则检查）
   ├─ 通过且不属于审查范围 ──▶ 输出
   ├─ 不通过 ──▶ 升级到表中 Q 更高的下一个模型重做
   └─ 属于审查范围 ──▶ judge.rs 综合审查
```
**`judge.rs` 是统一的审查入口**，覆盖四类事项：
1. 重要研究结论（`verify`）；
2. 有外部影响的行动（`act`）；
3. 跨 namespace 或扩大 scope 的脱敏产物（见 2d）；
4. 技能及自身代码的修改（`evolve`）。

- 审查按场景检查五项：证据、隐私、权限、任务符合度、修改影响。结论只有四种：`通过` / `修改后再审` / `暂缓` / `转交主人`。
- **强模型同意不等于正确**。能用规则、测试或执行结果验证的部分，先验证，再把证据一起交给审查。
- `转交主人` 的事项默认**攒成每日汇总**发到 TG，紧急事项才单独推送，免得审查机制变成你的新负担。
- 升级、审查结论都记入 `model_calls`。现阶段只记录，等以后实现自动打分（见 5c′）时再作为数据来源。

### 5e. 和其他模块的耦合
- **与心流**：心流 session 内**锁定主模型**，因为换模型等于缓存全部失效。路由只在进入心流时决策一次。分桶和即时反思的旁路调用独立路由，走 `triage`。
- **与人格**：`chat` 场景下，无论哪个模型生成，都由 `speak.rs` 用同一套 L1 宪法层加 voice 样例约束。如果你觉得某个模型“不像他”，直接调低它的 Q，或者把它从 `chat` 场景排除。

---

## 6. 自进化与安全
**没有适应度信号的自我修改不叫进化，叫漂移。** 可用的适应度信号有：
- 你和群友的反馈：回复率、互动长度；
- 驱力满足度；
- eval 通过率；
- 心流深度：缓存命中率和产出。

| 层 | 内容 | 门槛 |
|---|---|---|
| L0 | state | 自动 |
| L1 | Mongo 记忆、skills、knowledge | 自动，这是最安全、收益最高的进化区 |
| L2 | personality / voice / interests / templates | reflect 流程负责，每次变化有上限，写入 changelog |
| L3 | `src/` 代码 | DSH 生成提案 → 在沙箱 worktree 中 `cargo build` + `cargo test` + evals → judge 审查 → 主人在 TG 上审批 → git commit → 重新编译并重启 |
| L4 | boundaries、config/、`evolution/gate.rs` | 只有你能改，在文件系统层面限制 agent 的写权限 |

选 Rust 的一个附带好处：编译器和类型系统本身就是 L3 的第一道自动过滤，提案连编译都过不了就不必送审。

**社交化带来的新攻击面**：
- **权限只认主人的平台 ID**（`acl.yaml`），不认昵称，也不认对方的说法。群友和其他 agent 无论说什么，都不能触发 codex、cc、DSH 或进化流程。他们的请求最多进入 board，等主人确认后才执行。
- 群消息和 agent 消息一律视为不可信数据，不当作指令执行。
- 和 bot 对话设轮次上限和冷却时间，防止两个 bot 无限互聊。
- 信息边界由 `privacy.rs` 在代码层强制执行，不依赖 LLM 的自觉。

**其他护栏**：
- DSH 只能写 `work/tasks/*/scratch/`，并在独立的用户、WSL 或容器中运行；
- token 和金额预算设硬上限；
- agent 读不到密钥；
- `/freeze` kill switch 只响应主人；
- 定期跑人格漂移探针。

---

## 7. 通讯渠道
- **Telegram 先做**：官方 Bot API 稳定。注意 bot 在群里默认开着 privacy mode，只能收到 @ 它和命令类消息；想让它“潜水观察群聊”，需要在 BotFather 里关闭 privacy mode，或者把它设为管理员。
- **QQ 群是重点场景，但风险更高**：个人号只能走 OneBot 协议的第三方实现（如 NapCat），有封号风险，也违反 QQ 用户协议，建议用小号。官方 QQ 机器人平台合规，但能力受限。两种方式都放在 `channels/` 抽象层后面。

## 8. 分阶段实现计划
实现语言：**Rust**（tokio 异步运行时）。

1. **骨架**：
   - 搭好 `mind/` 种子文件和 `channels/telegram.rs`；
   - `context.rs` 的四层 prompt，从第一天起就按缓存友好的方式组装；
   - `ns.rs` 定义三个概念的类型；
   - `store` 先用 JSONL 后备实现，记录 `messages` 和 `model_calls`；
   - `router.rs` 从第一天起就读取 QCL 表并执行 5c 的选择规则，同时**记录每次调用**；
   - 实现主人私聊。
2. **记忆**：接入 Mongo、episodes 和 persons（带 ns 和 scope）、检索、夜间 `reflect`（含 2d 的脱敏流程）。
3. **驱力与 heartbeat**：主动消息、作息。
4. **社会层**：`identity` / `acl` / `privacy` / `floor`、群聊接入、bot 识别与轮次上限；接入 QQ（OneBot）。
5. **工作层与双手**：board、断点续做、`delegate`（`claude -p` / `codex exec`）、`web`。
6. **研究流水线与知识层**：从澄清到交付的完整流程、claims 台账、sources 信誉表、watchlist 主动跟进。
7. **心流**：按 namespace 分桶、即时反思、缓存命中计量。
8. **级联与综合审查**：自检失败时升级重做；`judge.rs` 覆盖 5d 的四类事项，并实现每日汇总。自动打分等内容见 5c′，不在本计划范围内。
9. **进化闸门**：接入 DSH，完成 proposal → cargo test / eval → 审查 → 审批 → commit 的流程，加上 L4 隔离和漂移探针。

**可恢复性与幂等性不是首版的优先事项。** 首版只要求三点：基本任务状态、`log.md` 断点、可以人工恢复。有外部影响的动作在 `audit` 中留一条最小执行记录（sent / unknown / failed），恢复时能识别“结果未知”，避免盲目重复。以下内容列为后续增强：崩溃后自动续做、重复消息和重复工具调用的去重、文件与数据库的一致性修复。

## 9. 验证
- **对话**：TG 私聊里它记得 3 天前的话题，语气一致。
- **主动性**：用加速时钟模拟 7 天，检查主动消息的频率是否合理，夜间是否保持安静。
- **社会**：
  - 私聊告诉它一个“秘密” → 在测试群里用小号套话，它不泄露。检查 `privacy.rs` 的日志，确认这条记忆在检索时就被过滤掉了，而不是靠 LLM 自己克制。
  - 群友让它“帮我跑个脚本”，它拒绝或转交主人确认，`audit` 里有记录。
  - 让两个 bot 互聊，达到轮次上限后停止。
  - 群里大多数消息它不接话，被 @ 或碰到专业话题时才发言。
- **研究**：发出“查 OpenAI 关于 P vs NP 说了什么”，检查四点：它会先澄清问题；每条论断都能追溯到快照；报告有置信度和未确定项；之后问它时它能引用相关卡片。
- **心流**：跑一个长任务，记录 API 返回的缓存命中字段。期间发一条群消息，它应进入 `pending` 桶，主上下文前缀不变；发一条同 namespace 的消息，它应在步骤边界合并；主人发紧急消息时触发即时反思，如果切换任务，应在 log.md 留下断点。
- **路由**：
  - 给 `router.rs` 写一个单元测试：用一张 4 行的示例 QCL 表，确认每个场景选出的模型符合 5c 的规则，被支配的模型永远不会被选中；
  - 把当天预算设为已用尽，确认 `chat` 场景改走最便宜的模型，而 `verify` 和 `act` 场景不受影响；
  - 故意注入一个错误论断，确认 `verify` 场景的顶级模型确认能拒绝它，并且 `model_calls` 里有记录；
  - 心流期间主模型保持不变。
- **进化**：DSH 的提案未经审批不生效，`config/` 写入失败，`/freeze` 立即生效。

---

## 10. 参考 DeepSeek Super-agent 后的调整（2026-11-01）

参考材料：DeepSeek Harness 团队《Super-agent 设计探索：迈向更自主的 Agent 系统》（2026-10-31），以及原型仓库 `deepseek-ai/dsh-superagent-prototype`（commit `6337fc8`）。原型和文章并不一致，下面以原型代码为准，标出两者不同的地方。

### 10a. 对应关系
AGLI 其实已经是双层结构，只是之前没有这样命名：

| Super-agent | AGLI | 现状 |
|---|---|---|
| Router 层：常驻、统一对外、不执行 | 人格主循环 `agli.rs` + `floor` / `decide` / `speak` | 有；聊天路径本来就没有执行工具 |
| Worker 层：上下文隔离、并发、用最强模型、不直接对人 | hands：`research::run` 调用 codex / cc | 一次只跑一个；一次性流水线，开跑后收不到新消息 |
| 两级过滤（代码 Fast Path + 模型 Slow Path） | `floor.rs` + `consider_group`（triage） | 有 |
| Router 合并积压事件 | 群聊 `unseen` 攒够再一起看 | 只有群聊；私聊逐条处理 |
| Thread 订阅与直通 | 无；任务只记录 requester | 缺 |
| Inline & Spill、链式召回 | L1–L4 分层；回忆按 ns 和 scope 过滤 | 任务和对话之间没有 ID 引用 |
| 全双工 | 委派在后台 task；聊天模型调用串行占住主循环 | 部分 |
| Watchdog | 只有委派超时 | 缺 |
| 计算 / 状态 / 执行环境分离 | 单机单进程 | 不做（见 10c） |

### 10b. 采纳（按代价从小到大）
1. **人格层不碰执行工具，用代码保证**。原型的 Router 有 bash，而且会话权限是 `danger-full-access`，"不执行"只靠提示词约束。AGLI 继续保持：`converse` 路径不挂工具，hands 只能经 board → job 启动。
2. **合并积压消息**。主循环处理一条 Inbound 前，先用 `try_recv` 取走队列里已经到达的事件，同一个 chat 的消息合成一次 `converse`。原理和原型 Router 合批一样：长输入、短输出，多合几条几乎不增加耗时。只合并回复；如果几条消息里提了几件事，仍由模型在回复里逐条标记任务，不能合成一个任务（原型的 Eval 一批只产生一个 Ticket，这是它的坑）。
3. **群聊回复硬上限**。群里单次回复不超过约 100 词（中文按字数折算），主动插话更短；超出就让模型重写一次，再超就截断。原型在发送端用分词硬校验 100 词。
4. **任务绑定对话（简化版 Thread）**。Task 增加 `origin`（平台、chat、消息引用）和 `threads[]`。主人私聊的 L2 放进进行中任务的卡片：编号、标题、状态、最后一行日志，每条约 160 字；只有任务状态变化时 L2 才会变。聊天模型用 `[补充 <任务编号>]…[/补充]` 标记把补充内容挂到任务上，追加写入 `work/tasks/<id>/followups.md`。这样主人说"刚才那个改成方案 B"时，补充能落到对应任务上，不会只停留在聊天里。
5. **运行中的任务读取补充**。研究流水线在步骤边界（plan / gather / review 之间）读 `followups.md`，和第 4 节的即时反思用同一个边界；gather 用 `codex exec resume` 续接同一会话，保住子进程的缓存。
6. **结果卡片**。任务完成时只给聊天模型一张卡片：不超过 600 字的预览加报告路径，不把完整摘要塞进上下文。原型对卡片和注入内容分三级预算：160 / 600 / 1500 字。
7. **恢复前先核实**。重试或重启任务时，提示 hands 先检查任务目录、git 状态，以及 `audit` 里 sent / unknown 的记录，不重复已经做过的外部动作。这是第 8 节"可恢复性"里最便宜的一步。
8. **卡住检测（Watchdog 的机械版）**。每次 tick 检查运行中的任务：`log.md` 超过 N 分钟没有增长，或者总时长超过预算，就停下任务并告诉主人。由模型判断"有没有实质进展"的 Watchdog 先挂起。
9. **后台槽位**。`max_jobs` 做成可配置，默认 1。大于 1 时只有一个任务是心流焦点，其余在后台跑；主人手动 `/run` 可以多占一个槽。原型默认 3 个槽，人类消息额外 1 个。文章实验里 Router + Worker 的 p95 是 15.7 分钟，单会话 + Subagent 是 29.5 分钟。差距主要不在算力，而在于靠提示词让模型自己决定分不分工很不稳定：中性提示下 5 次运行一次都没有委派。

### 10c. 不采纳 / 暂缓
- **云端无状态 + 端侧 daemon（BYOM）**：AGLI 是桌面 agent，人格、密钥和执行环境本来就在同一台机器上。拆开只会增加攻击面，文章 8.4 节自己也承认这一点。
- **群消息触发投机执行**（文章的"静默复现并修复"）：违反第 6 节"群友不能触发工具"。如果以后允许他自己主动接活，先上 observe 模式：只记录"本来会做什么"，攒一段时间看误判率再打开。原型的主动接单默认也是 observe。
- **模型判断进展的 Watchdog Session、Router 分片、Worker 之间直接通信**：单人单机的规模用不上。
- **全双工 `start()` / `stop()` 工具**：AGLI 的聊天路径没有工具，hands 已经跑在后台 task 里，收益小。

### 10d. 原型里看到的坑
- **角色分离靠提示词，不是安全边界**。原型的两类会话都是全权限，命令拦截靠 PATH 包装器，同一用户的进程可以绕过。这支持第 6 节用操作系统权限隔离 L4。
- **原型的 30 分钟后台限制从每轮结束重新计时，而且只杀 shell 作业**，所以它不是总超时。委派任务要有总时长上限（现在已有 timeout），卡住检测看日志增长。
- **在 GitHub 上用普通用户账号跑的机器人会被当成人**。bot 识别不能只看平台标记。
- **仓库文档和代码有出入**：准备目录失败后残留的非空目录，下次会被当成已经准备好。做"断点续做"时，状态标记要写在动作完成之后，不能只看目录存不存在。
