# 主人配置参考

初次核对：2026-11-02（工单 #69）；2026-11-15 更新已漂移的字段和边界，完整发现见[审查报告](reviews/2026-11-15/REVIEW.md)。本文按 `src/config.rs`、加载器和实际调用处整理。`config/` 属于 L4，由主人修改；这里仅提供参考，不代改配置。

## 怎样读这份表

- **必填**表示没有缺键默认值，删掉会导致反序列化失败；表中其他默认值均表示该键可省略。`无`表示 `Option::None`，不等于空字符串。
- `*` 是你起的映射名称，`[]` 是列表的一项。`usize/u64` 是非负整数，`i64` 是有符号整数，`f64` 是浮点数，路径用字符串，时间用带引号的 `HH:MM:SS`。
- **只有 `social.yaml` 支持热加载**；其中 `aliases` 仅作启动时的一次词库种子，改它不会更新已创建的词库。其余文件的字段均需重启。环境变量变更也应重启进程。
- 主程序约每 2 秒轮询配置，连续两次快照相同才尝试加载；加载失败保留旧配置。它会校验所有必需配置，因此其他文件的错误也可能阻止 social 热加载。已启动的后台任务持有启动时的配置快照。
- 六个现有 YAML 文件必须存在；`tools.yaml` 是唯一可缺失的文件。嵌套对象默认必填，只有明确标为可选的块可整块省略。Config.root 是由启动参数/环境确定的内部字段，不是 YAML 键。示例是需要合并进现有文件的片段，不是完整文件；不要重复创建同名顶层键。

## models.yaml（全部需重启）

依据：[配置类型](../src/config.rs)、[模型加载与预算](../src/mind/models.rs)、[路由](../src/mind/router.rs)、[请求与计费](../src/mind/llm.rs)。价格单位均为美元 / 百万 token，数值由主人维护，并不自动查询服务商报价。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `daily_budget_usd` | f64 | 必填 | 本地自然日路由阈值；耗尽后未固定模型的非豁免场景改走 cheap，固定模型的非豁免场景拒绝调用，embedding / 后台图片描述停止。不是全局硬上限，不含 DSH 费用 |
| `providers` | 名称 → 对象 | 必填 | 服务商表 |
| `providers.*.kind` | `anthropic` / `openai_compat` | 必填 | 请求协议 |
| `providers.*.base_url` | 可空字符串 | 无 | API 基址；Anthropic 未指定时用客户端内置官方基址，兼容接口必须能解析出基址 |
| `providers.*.base_url_env` | 可空字符串 | 无 | 基址环境变量名；非空环境值优先于 base_url，去除两端空白和末尾斜线 |
| `providers.*.api_key_env` | 可空字符串 | 无 | 密钥环境变量名；省略表示不需要密钥；指定但未设密钥的聊天模型不会进入路由 |
| `models` | 对象列表 | 必填 | QCL 模型表 |
| `models[].id` | 字符串 | 必填 | 内部模型 ID，用于路由、日志和会话锁定 |
| `models[].provider` | 字符串 | 必填 | 引用 providers 中的名称，未知名称启动失败 |
| `models[].model` | 字符串 | 必填 | 发送给 API 的模型名称 |
| `models[].q` | f64 | 必填 | 主人估计的质量分；按场景 q_min 过滤 |
| `models[].vision` | bool | false | 已实测能收图；`vision: true` 的场景只路由到这些模型 |
| `models[].c` | 对象 | 必填 | token 价格表 |
| `models[].c.input` | f64 | 必填 | 未缓存输入价格；路由成本比较使用 input + output |
| `models[].c.cached` | f64 | 必填 | 缓存命中输入价格，用于计费记录 |
| `models[].c.output` | f64 | 必填 | 输出价格 |
| `models[].c.cache_write` | 可空 f64 | 无；计费时取 input × 1.25 | 缓存写入价格；当前部分模型显式填写，0 也是有效值 |
| `models[].l` | f64 | 必填 | 首字延迟估计（秒）；fast 路由据此排序，不会自动用实测值覆盖 |
| `models[].local` | bool | false | 声明为本地处理，可处理 local_only；启动时检查本地服务可达性 |
| `models[].fallbacks` | 可空字符串 | 无 | Anthropic 服务端拒答回退参数；不是 AGLI 自己的升级链；当前仅部分模型填写 |
| `models[].stream` | bool | false | 兼容接口使用 SSE；内部收齐正文后发送，不向渠道转发推理增量 |
| `models[].enable_thinking` | 可空 bool | 无（不发该参数） | 兼容接口 thinking 参数；场景 effort 为 low 时实际发 false；当前仅部分模型填写 |
| `models[].reasoning_effort` | bool | false | 仅 openai_compat 协议按场景 effort 发 `reasoning_effort`；Anthropic 协议不使用此开关，只在场景提供 effort 时发 `output_config.effort`。当前 Gemini provider 已改用 Anthropic 协议 |
| `sticker_embedding` | 可空对象 | 无（不建表情包向量） | 字段同 `embedding`，只用到 provider、model、local；和语义召回分开，当前为本地 `bge-m3`，基址用 `127.0.0.1`（`localhost` 在这台机器上每次多 2 秒） |
| `embedding` | 可空对象 | 无（关闭语义召回） | **可选块，当前未配置**；独立于聊天模型列表 |
| `embedding.provider` | 字符串 | 块内必填 | 引用 providers，调用时要求 openai_compat |
| `embedding.model` | 字符串 | 块内必填 | embedding 模型名，调用基址下的 `/embeddings` |
| `embedding.local` | bool | false | 是否允许把 local_only 内容交给这个 embedding 模型；必须符合真实部署位置 |
| `embedding.input_usd_per_million` | f64 | 0.0 | 向量化输入计费，必须有限且非负 |
| `embedding.max_cross_namespace` | usize | 3 | 跨 namespace 语义命中上限，还受 episodes_in_context 剩余位置限制；0 不增加跨域命中 |
| `embedding.threshold` | f64 | 0.75 | 余弦相似度门槛，有限且在 [0,1] 内 |
| `embedding.nightly_cap` | usize | 200 | 每夜成功补建向量数量上限；0 关闭夜间补建，不关闭提炼后的建向量或查询 |

**记忆语义召回（#2）默认关闭；表情包向量另有 `sticker_embedding: bge-m3` 配置。** 配置本身不证明当前服务在线。本地 Ollama 的 `nomic-embed-text` 仍是下面的示例候选，启用前需确认服务、模型和接口。无需把 embedding 模型加入聊天 QCL 表。

```yaml
# models.yaml；沿用已有的 providers.local，候选未验证
embedding:
  provider: local
  model: nomic-embed-text
  local: true
  input_usd_per_million: 0
  max_cross_namespace: 3
  threshold: 0.75
  nightly_cap: 200
```

实现见 [embedding.rs](../src/mind/embedding.rs) 和 [privacy.rs](../src/social/privacy.rs)：不在启动时全量回填；提炼后和夜间渐进补建。先按 scope 过滤，再比较向量；同 namespace 记忆优先，跨域结果按相似度 × 衰减后显著性排序。接口失败保留普通召回。数值校验失败会停用 embedding 并警告；未知 provider、协议不匹配等会在调用时失败，配置存在不代表接口验证成功。

## scenarios.yaml（全部需重启）

顶层以场景名称为键。加载器要求 `chat`、`triage`、`extract`、`consolidate`、`research_plan`、`verify` 和 `synthesize`。调用处还使用 `work_review`（研究与编码任务逐轮审查）、`act`、`evolve`、`caption`（后台描述图片）和 `format`（发送前整理）。缺少 caption 时描述失败，下载文件可能仍保留。已有运行目录升级时，请把仓库的 `work_review` 行同步到运行目录的 `config/scenarios.yaml` 后重启。

`work_review.model` 默认精确指定百炼的 `bailian-deepseek-flash`，使用 `BAILIAN_API_KEY`。审查读取任务要求、材料和本轮产物，修改意见会进入下一轮。直接网页读取和文档解析独立使用官方 `DEEPSEEK_API_KEY`，由当前对话的处理策略决定能否提交。官方工具调用的账单应以 DeepSeek 平台记录为准。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `*.model` | 可空字符串 | 无（正常 QCL 路由） | 精确固定内部模型 ID；仍校验 policy、Q 下限及 vision。固定后不自动升级，非豁免场景超预算时拒绝调用 |
| `*.q_min` | f64 | 0.0 | 质量下限 |
| `*.priority` | `cheap` / `fast` / `quality` | 必填 | 帕累托过滤后，分别按成本、延迟、质量排序 |
| `*.review` | bool | false | 记录设计意图；**不自动接入 judge**，实际审查由调用处显式执行 |
| `*.budget_exempt` | bool | false | 超预算时仍保留原优先项；当前 verify / act / evolve 为 true |
| `*.effort` | 可空字符串 | 无 | Anthropic 发 output_config.effort；启用 reasoning_effort 的兼容模型发同名参数；low 还会关闭兼容模型显式配置的 thinking |
| `*.vision` | bool | false | 只路由到 `vision: true` 的模型，固定 `model` 时也检查；当前 `chat` 为 true 并固定到 `gemini-4-argon`，[看图](../src/mind/vision.rs)把图片直接放进被叫到的那一轮 |

当前部分场景省略 q_min 以外的可选字段，按上述默认值处理；effort 的字符串不在配置层校验。依据：[Scenario](../src/config.rs)、[router.rs](../src/mind/router.rs)、[llm.rs](../src/mind/llm.rs)、[judge.rs](../src/mind/judge.rs)。

仓库默认 `chat` / `caption` 固定到 Gemini Argon，`format` 固定到百炼 DeepSeek Flash，`evolve` 固定到 Opus。启动只验证聊天模型可选，不证明其他场景或提供商可用。另有已确认缺陷：只要默认本地 `qwen3:8b` 可用，日记/成长就选择 LocalOnly，但其 Q=5 不满足 consolidate 的 Q≥6；人格探针的 LocalOnly 又与远程 chat 固定项冲突。具体触发及修复边界见审查报告。

## runtime.yaml（全部需重启）

依据：[类型及校验](../src/config.rs)、[启动](../src/main.rs)、[主循环](../src/agli.rs)、[会话](../src/mind/speak.rs)。时间使用运行机器的本地时区；路径相对于项目根（AGLI_ROOT 或启动工作目录），绝对路径也可用。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `data_dir` | 路径 | 必填 | JSONL、状态、审查记录、导出人物卡和探针等运行数据；Rust 支持自定义，但现有 ops 脚本固定使用 data，尚未对齐 |
| `mind_dir` | 路径 | 必填 | 人格种子、日记、成长 overlay 等 |
| `work_dir` | 路径 | `work` | 默认看板与任务分别写入运行根目录下的 `work/board.yaml` 和 `work/tasks/<id>/`。显式相对路径也从运行根解析，绝对路径直接使用 |
| `session` | 对象 | 必填 | 会话与连发合并设置 |
| `session.max_turns` | usize | 必填 | 已存消息达到 2 × 此值时，下次对话重建会话 |
| `session.idle_minutes` | i64 | 必填 | 空闲超过此分钟数，下次对话重建会话 |
| `session.recap_messages` | usize | 必填 | 重建会话 L2 时读取的最近聊天消息条数 |
| `routine` | 对象 | 必填 | 本地作息 |
| `routine.sleep_at` | 时间 | 必填 | 入睡边界；进入睡眠时触发夜间反思 |
| `routine.wake_at` | 时间 | 必填 | 醒来边界；睡眠期间不主动联系，紧急通知也延后 |
| `heartbeat` | 对象 | 必填 | 内在时钟 |
| `heartbeat.tick_s` | u64 | 必填 | tick 秒数；主程序实际至少 10 秒，watchdog 也在 tick 检查 |
| `heartbeat.proactive_gap_hours` | i64 | 必填 | 主动联系主人的最小间隔（小时） |
| `judge` | 对象 | 必填 | 审查汇总 |
| `judge.digest_at` | 时间 | 必填 | 当天达到此本地时间后的 tick 尝试每日汇总，并非精确闹钟 |

### session.debounce：回复前合并连发

**可选整块，当前已显式配置全部字段。** 不写时也默认启用。按平台和 chat 学习间隔，估计值保存在 KV；命令绕过等待。依据：[事件队列与自适应窗口](../src/agli.rs)。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `session.debounce` | 对象 | 下列默认值 | 同一聊天连发合成一次回复 |
| `session.debounce.quiet_ms` | u64 | 2500 | 无学习样本时的静默等待毫秒；0 同时关闭等待与学习 |
| `session.debounce.cap_ms` | u64 | 10000 | 从首条消息到放行的最大等待毫秒；启用时必须 > 0 |
| `session.debounce.adaptive` | bool | true | 根据聊天节奏自适应；false 固定使用 quiet_ms |
| `session.debounce.alpha` | f64 | 0.3 | 新间隔在指数滑动平均中的权重，启用自适应时须在 (0,1] |
| `session.debounce.factor` | f64 | 2.5 | 平均间隔乘数，须为正且有限 |
| `session.debounce.min_ms` | u64 | 1200 | 自适应静默窗下限 |
| `session.debounce.max_ms` | u64 | 6000 | 自适应静默窗上限 |

自适应开启时要求 `0 < min_ms <= max_ms <= cap_ms`；间隔超过 cap_ms 不用于学习。quiet_ms 为 0 时直接跳过此块其余校验。

```yaml
session:
  # 保留现有 max_turns / idle_minutes / recap_messages
  debounce:
    quiet_ms: 2500
    adaptive: false  # 固定等待，其余键沿用默认值
```

### memory：召回、遗忘归档与周反思（#2 / #3 / #5 / #28）

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `memory` | 对象 | 必填 | 记忆设置 |
| `memory.extract_after` | usize | 必填 | 一个聊天未提炼消息达到此数触发提炼；夜间也会提炼 |
| `memory.episodes_in_context` | usize | 必填 | 新会话 L2 中经历总数上限，含语义召回 |
| `memory.facts_in_context` | usize | 必填 | **每位对话伙伴**最多纳入的事实条数，不是所有人的总上限 |
| `memory.weekly_weekday` | 星期枚举 | `Sun` | **可选、当前未写**；周反思和人格探针的周边界，可用 Mon/Tue/Wed/Thu/Fri/Sat/Sun |
| `memory.half_life_days` | f64 | 30.0 | **可选、当前未写**；显著性半衰期（天），须为正且有限 |
| `memory.archive_threshold` | f64 | 0.05 | **可选、当前未写**；衰减后显著性严格低于此值才可归档，须在 [0,1] |
| `memory.archive_min_age_days` | f64 | 30.0 | **可选、当前未写**；年龄严格大于此天数才可归档，须非负且有限 |
| `memory.archive_max_groups` | usize | 20 | **可选、当前未写**；每夜归档尝试的分组数上限，失败尝试也占用；0 关闭归档 |

显著性按 `salience × 0.5^(年龄天数 / half_life_days)` 在读取时计算，不覆盖原始 salience。夜间按相同 namespace 与 scope 分组摘要，成功后原始经历退出活动召回但仍保留；已有归档摘要不再反复摘要。只读群归档与只读反思共享金额上限，ignored_groups 不参与。依据：[reflect.rs](../src/mind/reflect.rs)、[存储](../src/store/mod.rs)。

weekly_weekday 按反思日记日期划分周窗口（本地时间减去 6 小时后取日期），不是“只有当天才能执行”：有日记且本周尚未成功时，夜间会补跑；探针在本周成长成功后运行。数值成长每周单项最多 ±0.02、相对种子累计最多 ±0.15，属于代码固定限制，不是配置键。见 [growth.rs](../src/mind/growth.rs)、[probes.rs](../src/mind/probes.rs)。

```yaml
memory:
  # 保留现有 extract_after / episodes_in_context / facts_in_context
  weekly_weekday: Sun
  half_life_days: 30.0
  archive_threshold: 0.05
  archive_min_age_days: 30.0
  archive_max_groups: 20
```

### hands：执行器、槽位和超时

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `hands` | 对象 | 必填 | 委派设置 |
| `hands.max_jobs` | usize | 1 | **可选、当前未写（#37）**；必须 >= 1。大于 1 时只有一个心流焦点，其他任务后台运行；主人手动 /run 可额外占一个槽。等于 1 时无额外槽 |
| `hands.research` | `codex` / `dsh` / `off` | `codex` | 可选；**当前显式设为 dsh**。研究资料搜集执行器，off 关闭外部搜集，不等于关闭所有研究规划模型调用 |
| `hands.code` | 同上枚举 | 必填 | 编码仅支持 codex / off；写 dsh 会在加载配置时报错 |
| `hands.codex_model` | 字符串 | 必填 | 固定 Codex 模型，不跟随其用户配置默认值；研究走 dsh 时仍是必填项 |
| `hands.dsh_profile` | 字符串 | `agli` | **可选、当前未写**；DSH profile 名称，模型与工具权限由该 profile 管理 |
| `hands.dsh_home` | 路径 | `data/dsh-home` | **可选、当前未写**；启动时相对项目根解析为绝对路径，供 DSH_HOME 使用；不跟随 data_dir 自动变化 |
| `hands.timeout_min` | u64 | 必填 | 每次执行器子进程调用的总超时分钟数，涵盖 stdin/stdout/stderr 与等待退出；当前 YAML 为 30 |
| `hands.stall_minutes` | u64 | 20 | **可选、当前未写**；任务 log.md 无增长或更新达到此分钟数，watchdog 暂停任务 |
| `hands.total_budget_minutes` | u64 | 90 | **可选、当前未写**；一次运行任务的总时长上限，watchdog 暂停；跨多次执行器调用累计 |

timeout 是单次子进程期限，watchdog 是主循环按 tick 检查的任务级期限；主循环中的慢模型调用还会推迟检查。三项分钟数均在配置加载时要求大于 0。DSH 不是安装后就自动就绪：需要独立 home/profile 和专用凭据；详细操作见 [DSH.md](DSH.md)。实现见 [delegate.rs](../src/hands/delegate.rs)、[research.rs](../src/work/research.rs)、[agli.rs](../src/agli.rs)。

```yaml
hands:
  research: dsh
  code: codex
  codex_model: gpt-6-astra
  timeout_min: 30
  max_jobs: 1
  dsh_profile: agli
  dsh_home: data/dsh-home
  stall_minutes: 20
  total_budget_minutes: 90
```

### mongo：可选存储块，当前未写

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `mongo` | 对象 | 下列默认值 | 可整块省略，仍会读取默认环境变量名 |
| `mongo.url_env` | 字符串 | `MONGO_URL` | Mongo 连接环境变量名；未设或去空白后为空则用 JSONL |
| `mongo.database_env` | 字符串 | `MONGO_DATABASE` | 数据库名环境变量名；未设或为空时使用内置数据库名 aglibot |

```yaml
mongo:
  url_env: MONGO_URL
  database_env: MONGO_DATABASE
```

指定 Mongo URL 后连接失败会报错，不会静默切回 JSONL（[store::open](../src/store/mod.rs)）。`mongo.url` 和 `mongo.database` 是内部解析结果，不是可编辑 YAML 键；设计树中的 `mongo.yaml` 目前没有加载器。

## social.yaml（运行设置热加载，aliases 仅作一次启动种子）

依据：[social 类型](../src/config.rs)、[热加载](../src/config_watch.rs)、[群聊处理](../src/agli.rs)、[轮次限制](../src/social/floor.rs)、[反思预算](../src/mind/reflect.rs)。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `groups` | 字符串列表 | `[]` | 可以发言的群白名单，格式 platform:chat_id；不在名单且未忽略的已加入群默认只读记录 |
| `ignored_groups` | 字符串列表 | `[]` | **可选、当前未写**；跳过新输入、按群提炼与归档，不回复；优先于 groups。不会删除旧记录或从所有历史记忆派生流程中撤回它们 |
| `aliases` | 字符串列表 | `[]` | 启动时一次性导入空词库的全局称呼种子；已导入后修改此值或重启均不覆盖现有词库，词库由 Store 管理 |
| `private_from_others` | bool | 必填 | 是否接受非主人私聊；不授予主人权限 |
| `group_reply_max` | usize | 150 | **可选、当前未写**；群聊完整回复长度上限；中文按字、其他连续字母/数字按段计数 |
| `interjection_max` | usize | 80 | **可选、当前未写**；主动插话更短的长度上限；超限先重写一次，再截断 |
| `floor` | 对象 | 必填 | 接话节奏 |
| `floor.check_every` | usize | 必填 | 未回复群消息达到此数才请 triage 判断是否插话；被 @ / 回复走直接接话路径，仍受权限与限流约束 |
| `floor.unsolicited_cooldown_min` | i64 | 必填 | 同一群两次主动插话的最短间隔分钟数；心流忙碌时不主动插话 |
| `human_limit` | 对象 | 必填 | 人类回复限制 |
| `human_limit.max_replies` | usize | 必填 | 每个 (chat, sender) 在窗口内的最多回复次数 |
| `human_limit.window_min` | i64 | 必填 | 滑动窗口分钟数 |
| `agent_limit` | 对象 | 必填 | bot / agent 回复限制 |
| `agent_limit.max_replies` | usize | 必填 | agent 在同一聊天中的回复次数上限 |
| `agent_limit.window_min` | i64 | 必填 | agent 滑动窗口分钟数 |
| `read_only_reflect` | 对象 | 下列默认值 | **可选整块，当前未写**；只读群夜间反思预算 |
| `read_only_reflect.max_messages_per_night` | usize | 400 | 每夜所有只读群累计处理消息上限，0 不提炼其消息 |
| `read_only_reflect.max_cost_usd` | f64 | 0.10 | 每夜只读群反思与归档共享金额上限；按已记录花费决定是否准入下一次调用，进行中调用可能超出余额 |

```yaml
# social.yaml；保留其他必填设置
ignored_groups: []
group_reply_max: 150
interjection_max: 80
read_only_reflect:
  max_messages_per_night: 400
  max_cost_usd: 0.10
```

当前 YAML 顶部“不在名单里的群，消息一律不读不记”的旧注释与实现不符；要停止一个群的新增输入及按群提炼/归档，请用 ignored_groups。该设置不会撤回已保存的历史产物。

## acl.yaml（全部需重启）

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `owner` | 对象 | 必填 | 主人身份 |
| `owner.person_id` | 字符串 | 必填 | 主人的内部人物 ID |
| `owner.telegram_user_id_env` | 字符串 | 必填 | Telegram 数字用户 ID 的环境变量名；环境未设时内部 ID 为 0，此渠道无人拥有主人权限 |
| `owner.qq_user_id_env` | 可空字符串 | 无 | QQ 数字用户 ID 的环境变量名；键或环境缺失时内部 ID 为 0；当前已配置变量名 |
| `known_agents` | 字符串列表 | `[]` | 已知 bot / agent 的平台 ID，如 qq:123456；参与身份判定和 agent 限流 |

ID 的非空环境内容必须能解析为 i64，否则加载失败。`owner.telegram_user_id`、`owner.qq_user_id` 是内部字段，不能通过同名 YAML 键设置。依据：[resolve_env](../src/config.rs)、[identity.rs](../src/social/identity.rs)。

## channels.yaml（全部需重启）

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `telegram` | 对象 | 必填 | Telegram 配置；没有 enabled 键 |
| `telegram.poll_timeout_s` | u64 | 必填 | Bot API 长轮询超时秒数 |
| `telegram.proxy_env` | 可空字符串 | 无 | 代理环境变量名；未设或空则不显式配置代理；当前已填写变量名 |
| `qq` | 对象 | 必填 | OneBot v11 正向 WebSocket 配置 |
| `qq.enabled` | bool | 必填 | 是否启动 QQ 渠道；还需要 URL 环境值 |
| `qq.url_env` | 字符串 | 必填 | WebSocket URL 环境变量名；即使 enabled=false，该键仍须存在 |
| `qq.token_env` | 字符串 | 必填 | access token 环境变量名；环境缺失时不带 token，但变量名键仍必填 |

Telegram token 从固定变量 TELEGRAM_BOT_TOKEN 读取；无 token 或初始化失败则不启用该渠道。两条渠道都不可用时程序退出。Telegram 代理作用于其 HTTP 客户端，不是模型客户端的代理配置。`telegram.proxy`、`qq.url`、`qq.token` 是内部解析字段，不能在 YAML 中直接赋值。依据：[main.rs](../src/main.rs)、[telegram.rs](../src/channels/telegram.rs)、[qq.rs](../src/channels/qq.rs)。

## Telegram 运行通知

主人的 Telegram 私聊是固定收件处。启动时读取 `acl.owner.telegram_user_id_env` 指定的用户 ID（仓库默认 `OWNER_TELEGRAM_ID`），并使用现有的 `TELEGRAM_BOT_TOKEN` 和 Telegram 代理。机器人须能向该私聊发送消息。即使主人最近使用 QQ 或机器人处于冻结状态，通知仍走 Telegram。

| 代码 | 说明 |
|---|---|
| E1001–E1005 | 依次对应启动配置、数据存储、模型初始化、机器人运行状态初始化、无可用聊天渠道 |
| E2001–E2004 | Telegram 身份验证、Telegram 收消息、QQ 连接、聊天消息投递 |
| E3001–E3005 | 生成聊天回复、网页读取、文件读取、任务执行、审查模型调用。E3005 对应的事项会转交主人审查 |
| E4001–E4004 | 运行事件处理、运行状态保存、后台作业、进化重启 |
| S1001 / S1002 | 已冻结 / 已解冻。冻结保留日常聊天，解冻后恢复正常调度 |

通知中有可直接发送的 `/freeze`、`/unfreeze` 命令。故障发生时先记本机日志，再由独立队列向主人发送固定的中文描述。同码 60 秒内合并，冻结、解冻的实际状态切换逐次通知。Telegram 暂不可达或投递结果不确定时，本机日志记录未确认投递；主人 ID 或通知通道无法解析时只能通过本机日志排查。

## tools.yaml（可选文件，全部需重启；#16）

**当前文件不存在。** 由 [flow_metrics.rs](../src/mind/flow_metrics.rs) 独立加载，而非 Config::load。它只描述服务商缓存策略，用于心流恢复时的 debug 提示；不会修改请求 TTL、预热缓存、改变路由，也不会按 min_tokens 拒绝调用。

| 键路径 | 类型 | 缺键默认 | 用途 |
|---|---|---|---|
| `provider_cache` | 名称 → 对象 | 空覆盖表，保留内置策略 | 可选块；按 provider 名称匹配，未匹配才按协议种类回退 |
| `provider_cache.*.ttl_seconds` | 可空 u64 | 保留该名称的内置值；新名称为无 | 缓存有效期提示（秒）；0 明确设为自动管理、无到期提示；null 与省略均不覆盖 |
| `provider_cache.*.min_tokens` | 可空 u64 | 保留该名称的内置值；新名称为 64 | 最小缓存 token 数元数据，只出现在提示里 |

内置名称 `anthropic` 为 ttl_seconds=300、min_tokens=1024；`deepseek` 和 `openai_compatible` 为自动管理、min_tokens=64。注意这里的回退名称是 **openai_compatible**，而 models 的协议枚举是 **openai_compat**。为新 provider 写部分覆盖时，从通用默认值（无 TTL、64）开始，不继承其协议的内置值。

```yaml
provider_cache:
  anthropic:
    ttl_seconds: 300
    min_tokens: 1024
  bailian:
    ttl_seconds: 0
    min_tokens: 64
```

## 环境变量：只列名称，不展示任何实际值

本清单来自 `src/` 中 env::var、var_os、vars_os、环境注册表和配置内的变量名；未读取 `.env`。下面代码形式的内容都是**变量名**。变量用于什么由旁边的说明给出。进程启动会调用 dotenvy 加载环境；修改后重启。服务商基址和密钥虽在请求时再次读取进程环境，也没有重新读取磁盘环境文件的热加载机制。

| 名称 | 用途 / 读取位置 |
|---|---|
| `AGLI_ROOT` | 主程序选择运行时根目录 |
| `AGLI_SOURCE` | 源码 checkout；缺省等于运行时根。原供进化闸门使用，闸门退役后代码只解析、不再读取 |
| `AGLI_LOG_FILE` | 将 bot 标准日志同时写入指定文件 |
| `RUST_LOG` | tracing 的默认环境日志过滤器（间接通过库读取） |
| `TELEGRAM_BOT_TOKEN` | Telegram bot 凭据 |
| `OWNER_TELEGRAM_ID`、`OWNER_QQ_ID` | 当前 acl.yaml 指定的主人数字身份变量名 |
| `TELEGRAM_PROXY` | 当前 channels.yaml 指定的 Telegram 代理变量名 |
| `ONEBOT_URL`、`ONEBOT_TOKEN` | 当前 channels.yaml 指定的 QQ URL 与凭据变量名 |
| `ANTHROPIC_API_KEY`、`DEEPSEEK_API_KEY`、`BAILIAN_API_KEY` | 当前 models.yaml 指定的 AGLI 模型凭据变量名 |
| `ANTHROPIC_BASE_URL` | 当前 models.yaml 指定的 Anthropic 中转基址覆盖变量名 |
| `BAILIAN_BASE_URL_OAI` | 当前 models.yaml 指定的服务商基址覆盖变量名 |
| `MONGO_URL`、`MONGO_DATABASE` | 默认 Mongo 变量名；import-jsonl 工具也直接读取这两个固定名称 |
| `AGLI_DSH_BIN` | DSH 可执行文件覆盖；不设时从 PATH 查找平台对应的 dsh 启动程序 |
| `GEMINI_RELAY_BASE_URL`、`GEMINI_RELAY_API_KEY` | Gemini 中转的基址和密钥；仓库默认的 `chat` 要求 vision，目前只有这些模型可选，缺少时启动失败 |
| `AGLI_DSH_BAILIAN_API_KEY` | DSH 专用凭据，经注册表授权后映射为子进程变量 `DSH_BAILIAN_API_KEY` |
| `AGLI_DSH_DEEPSEEK_API_KEY` | DSH 专用凭据，经注册表授权后映射为子进程变量 `DEEPSEEK_API_KEY` |

所有 `*_env` 配置都允许主人换一个变量名，因此代码实际读取的名称集合取决于配置。不要把 AGLI 自身 provider 的 api_key_env 改成上述 DSH 专用变量来共享：注册表保护 AGLI 配置凭据的规则优先，会阻止它再转交执行器。

[delegate.rs](../src/hands/delegate.rs) 还枚举父进程环境，经过 [secrets.rs](../src/ns/secrets.rs) 筛选后仅继承必要运行变量。允许名单名称为：

```text
PATH SystemRoot windir COMSPEC PATHEXT TEMP TMP TMPDIR
USERPROFILE HOMEDRIVE HOMEPATH HOME APPDATA LOCALAPPDATA
ProgramData ProgramFiles ProgramFiles(x86) ProgramW6432 CODEX_HOME
HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY
http_proxy https_proxy all_proxy no_proxy
```

允许名单仍受秘密检测和注册表保护约束，并非无条件转发。`DSH_HOME` 是根据 hands.dsh_home **设置给子进程**，不是采用父进程同名值。`CARGO_MANIFEST_DIR` 是测试中编译期 env! 名称，不是生产运行设置；测试中的临时变量和 temp_dir/current_dir 等系统路径查询不属于主人配置。

## 核对时发现的行为差异

- social.yaml 的旧白名单注释、scenarios.yaml 的 review/effort 注释均不能完整反映实现；实际行为见上表。
- daily_budget_usd 对未固定模型的一般路由是降档阈值，固定且非豁免的场景则停止；全局硬预算仍是设计目标。只读反思金额上限也是按记录准入，可能被最后一次调用越过。
- （已修正）hands.code: dsh 与 timeout_min / stall_minutes / total_budget_minutes 为 0 现在在加载配置时报错。facts_in_context 实际按人限制。
- embedding 的 provider 能力不在启动时验证；tools 的缓存参数仅供观测，新名称部分覆盖也不继承协议策略。
- Config 的多数结构未启用 deny_unknown_fields，拼错键或给 serde(skip) 内部字段赋值可能被忽略。请使用本文列出的键，不依赖“能解析”证明生效。
