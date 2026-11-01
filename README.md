# AGLIbot

AGLI（Artificial Guy-Like Intelligence）：一个像受过良好教育的普通人那样生活的桌面 agent。设计见 [docs/DESIGN.md](docs/DESIGN.md)。

现在是第一版：Telegram 和 QQ（NapCat）双通道，私聊和群聊，有记忆、作息、驱力，会主动找你，能接研究和编码任务并交给 Codex 去做（Claude Code 只归主人用，他不调用）。
还没做的部分记在 Moldloft 的图谱里（见下文“开发管理”）。

## 运行

1. Telegram：用 [@BotFather](https://t.me/BotFather) 创建一个 bot，拿到 token。想让他在 TG 群里“潜水”，要在 BotFather 里关掉 privacy mode。
2. QQ：启动 NapCat，在 WebUI 的网络配置里加一个**只监听 127.0.0.1** 的 WebSocket 服务器并设置 token。个人号走的是非官方协议，有封号风险，用小号。
3. `cp .env.example .env`，填：
   - `TELEGRAM_BOT_TOKEN`、`ONEBOT_URL` / `ONEBOT_TOKEN`（至少一个通道）；
   - 至少一个模型提供商的密钥；
   - `OWNER_TELEGRAM_ID`、`OWNER_QQ_ID`：你在两边的数字 ID。不知道的话先留空启动，给他发条消息，日志会打印出来。
4. 启动：

```bash
cargo run
```

百炼 Token Plan 使用 `BAILIAN_API_KEY` 和 `BAILIAN_BASE_URL_OAI`，支持 `.env` 或进程环境变量；
环境变量优先，地址未设置时使用北京 Token Plan 地址。Windows 新增用户环境变量后，需要重启终端或启动 bot 的应用。
这里走 OpenAI 兼容接口，`BAILIAN_BASE_URL_ANT` 不需要配置。
默认加入 Qwen Flash / Plus / Max 和百炼 DeepSeek Flash / Pro，具体模型 ID 在 `config/models.yaml` 中修改；
只有百炼密钥也能覆盖所有场景。Qwen 的思考模式跟着场景走：`effort: low` 的场景（聊天、分诊）关闭思考，其余开启；推理内容不会发出去。
Token Plan 条目的 `c` 是路由估值，日志中的美元花费也是估算，不代表套餐实际扣费或剩余额度。

Telegram 的收发消息走环境变量 `TELEGRAM_PROXY` 指定的代理（例如 `http://127.0.0.1:7890`），留空则不走显式代理。
主人 ID、代理、OneBot token 这类因人因机器而异的值都放在 `.env`（已被 gitignore），`config/` 里只写变量名。

## 他的一天

- **聊天**：私聊里每条都回；群聊只在被 @、被回复，或者攒了一些消息后用廉价模型判断“值得插话”时才说，大多数时候潜水。
  只参与 `config/social.yaml` 里列出的群，其他群的消息不读不记。
- **记忆**：聊天记录攒够一定量或夜里入睡时，提炼成经历、关于人的事实和去标识化的经验。
  每条记忆带可见范围（scope），检索时按当前听众过滤：私聊里你说的事不会出现在群聊的上下文里。
- **作息**：`config/runtime.yaml` 的 `routine` 决定睡觉时间。睡着时只有你能叫醒他；入睡时整理记忆并写日记（`mind/life/journal/`）。
- **主动性**：社交需求、好奇心随时间增长，越过阈值或有新任务时，他用廉价模型决定做什么：找你聊几句、开始一个任务，或者什么都不做。
  主动找你至少间隔 `proactive_gap_hours`。
- **工作**：你请他查东西或写代码时，他会在回复里答应并记到看板（`work/board.yaml`）。他决定开工后，先写计划，
  再交给 Codex 在 `work/tasks/<id>/` 里取证、写 `claims.yaml` 和报告；来源快照记 SHA-256；
  之后经过规则检查和审查模型复核，最后用他自己的话告诉你结论。
- **审查**：需要你拍板的事（别人请他动用工具、审查模型拿不准的结论）攒成每日汇总，`judge.digest_at` 时发给你。

## 主人命令（私聊里发）

`/status` 状态 · `/board` 看板 · `/task 描述` 新研究任务 · `/code 描述` 新编码任务 · `/run 编号` 立即开始 ·
`/stop` 停下当前任务 · `/freeze` / `/unfreeze` 冻结主动行为和工具 · `/digest` 现在发待决事项 ·
`/approve 编号` / `/reject 编号 理由` · `/reflect` 现在整理记忆

## 你会经常改的文件

- `config/models.yaml`：QCL 表，平时只需要调 `q` 这一列
- `config/social.yaml`：他参与哪些群、插话频率、对 bot 的回复上限
- `config/runtime.yaml`：作息、心跳、记忆、委托工具与预算、每日汇总时间
- `mind/self/*`：人设草稿，改完重启生效（它属于 L1 宪法层）
- `mind/social/people/owner.md`：你希望他知道的关于你的事；`mind/social/groups/<平台>_<群号>.md`：某个群的介绍（可选）

运行数据写在 `data/`（已被 gitignore）：消息、模型调用、经历、经验、人物、审查队列、审计、状态。
`work/tasks/`、`work/board.yaml`、`mind/life/journal/` 含个人内容，也已 gitignore。

## 开发管理（Moldloft）

[Moldloft](https://github.com/FezzyZHANG/Moldloft) 以 submodule 形式放在 `Moldloft/`，记录待办 ticket 和决策笔记：

```bash
python Moldloft/tools/project.py run --project .
```

打开 http://127.0.0.1:8787/graph 。agent 只能提交提案，在图谱页面由你审批后才生效。
