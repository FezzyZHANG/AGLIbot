# L3 进化闸门（#26）

主人通过 Moldloft 确认范围并指定实现者；实现者交付补丁后，主人在 AGLI 私聊发送
`/evolve <id>`。闸门将补丁应用到独立 detached worktree，运行构建、测试和指定 eval，
调用 `evolve` 场景审查，最后等主人 `/approve <审查编号>`。AGLI 的模型和 hands 没有进化审批工具。

本次初始实现由主人明确授权 Astra。运行时拒绝修改 `config/`、boundaries、根目录
`AGENTS.md`，以及整个 `src/evolution/` 和 `src/agli/evolution.rs` 闸门实现。
worktree 提供版本和工作目录隔离，并不提供 OS 权限隔离。

## 提交候选

1. 在 Moldloft 建立无未完成前置依赖的工单，或先由主人接受一项 Moldloft proposal。
   主人在写代码前指定实现者。闸门只读取本项目 `.moldloft/project.json` 的端口，通过
   `127.0.0.1` 读取对应 Drydock；不会申请或使用 Moldloft 的批准权限。
2. 从明确的完整 commit ID 开始实现，导出 `git diff --binary` 格式补丁。
   新文件必须纳入 diff；不要交付仅修改了工作区、却未包含新文件的补丁。
3. 在运行配置的 `data_dir/evolution/inbox/<id>/` 放 `proposal.json` 和 `change.patch`。
   默认路径为 `data/evolution/inbox/<id>/`；ID 为 1–64 位小写字母、数字或连字符。
   这不是 hands 的 `work/tasks/`，须由主人或主人选择的实现者交付。
4. 主人在机器人私聊发送 `/evolve <id>`，显式确认清单中的来源、作者和范围。

最小 `proposal.json`（替换 commit 和 eval 名称）：

```json
{
  "source": { "drydock": "default", "ticket": 123 },
  "summary": "Describe the concrete behavior change",
  "base_commit": "0123456789012345678901234567890123456789",
  "writer_model": "gpt-6-astra",
  "writer_host": "codex",
  "files": ["src/example.rs"],
  "eval_tests": ["example::tests::specific_behavior"],
  "director": null
}
```

Moldloft proposal 来源改用 `"proposal": "<proposal-id>"`，不同时填写 `ticket`；
proposal 必须已 `applied`。ticket 必须存在、未关闭、没有未完成前置依赖。
闸门记录来源内容和版本；批准后提交前重新读取，变化即停止，要求新提案。

作者白名单只有以下组合。身份由主人核实并在清单中确认；清单不是模型身份的密码学证明。

| writer_model | writer_host | 条件 |
|---|---|---|
| `gpt-6-astra` | `codex` | 直接实现 |
| `opus` | `claude-code` | 主人使用 Claude Code；AGLI 不调用 Claude CLI |
| `deepseek-v4.1-flash-1017` | `dsh` | 必须有 Astra/Opus 的指导与 diff 审查记录 |

`opus` 是此协议对主人所用 Opus 的规范名称。其他组合在应用补丁和构建前拒绝。
定向实现另填：

```json
"director": {
  "model": "opus",
  "plan": "The director's implementation plan; files and deciding tests are listed above",
  "interfaces": ["Name the interfaces and intended changes"],
  "reviewed_patch_sha256": "SHA256 of the exact change.patch bytes",
  "review": "The director's review of the resulting diff"
}
```

指导者必须是 `gpt-6-astra` 或 `opus`。计划、接口、完整文件范围、判定测试、最终
diff 审查和补丁散列缺一不可。歧义应交还指导者；不能扩大清单范围。

## 检查和批准

只接受 `src/`、`tests/`、`evals/`、`docs/`、`Cargo.toml`、`Cargo.lock`、`README.md`
内的普通文件，排除上述闸门文件。路径使用 ASCII 和 `/`；拒绝路径穿越、Windows
别名、符号链接、子模块及未列入范围的文件。补丁上限 256 KiB。

闸门在 `data_dir/evolution/runs/<id>/` 创建 detached worktree，依次执行：

```text
cargo build --locked --offline
cargo test --locked --offline
cargo test --locked --offline <eval-name> -- --exact --list
cargo test --locked --offline <eval-name> -- --exact --nocapture
```

每项 eval 必须找到并实际通过一个非 ignored Rust 测试。没有测试、测试失败、编译失败
都不进入可批准状态。依赖须预先在本机 Cargo 缓存中准备好；闸门不自动下载依赖。
每个命令最多 30 分钟，每个输出流最多 4 MiB。子进程仅继承工具链所需环境变量，
不继承机器人、模型或委派凭据。

检查通过后，`evolve` 审查收到来源、作者/指导者、文件范围、完整 diff、tree ID、
补丁 SHA-256 和检查记录。只有明确的 `pass` 进入 `awaiting_owner`；`revise`、`defer`、
`owner` 或审查失败都要求修改/排查后用新 ID 重走检查，主人不能用一次批准跳过失败。

- `/evolve-status <id>`：查看状态、提交与重启回执。
- `/digest`：查看待审批记录。
- `/approve <审查编号>`：只在主人私聊生效，冻结期间不可批准；同一审批不能重复使用。
- `/reject <审查编号> 理由`：拒绝候选，保留证据。
- `/stop`、`/freeze`：取消当前进化任务，并结束构建/测试进程树。

批准后再次比对来源、HEAD、索引 tree 和工作文件，拒绝过期证据。闸门使用
`commit-tree` 提交审查过的 tree，不执行 Git hooks，创建 `codex/evolution-<id>` 分支。
主人的 checkout 和其中未提交的文件保持原状。随后从这个 commit 在全新的
`data_dir/evolution/releases/<id>/` 目录运行 `cargo build --locked --offline --release --bin aglibot`。

重启助手记录意图，等旧机器人退出，再校验 release 可执行文件 SHA-256 并启动新版本。
新进程继续使用原来的 `AGLI_ROOT`、配置和数据目录；加载存储、模型与通道后，在进入
主循环前写 `running` 回执（不代表所有外部服务均已连通）。新版本的源代码分支保存在进化分支中；主 checkout
不会自动 merge。后续开发应使用该分支/commit 为基线，避免丢失已启用的修改。
新进程确认运行后会更新运行时根的 `data/release.json`；手动部署（`ops/deploy.ps1`）在目标提交不包含这个进化提交时会拒绝，避免悄悄回滚。闸门的 git 操作和 Moldloft 读取在源码仓库（`AGLI_SOURCE`）进行，运行数据和 release 在运行时根。见[部署](DEPLOY.md)。

## 证据和中断

运行记录存于现有 Store 的 `evolution:<id>`；审查队列保留作者、指导者、来源、tree、
散列和检查记录。构建日志、`restart-request.json`、`restart.json`、新进程的
`runtime.log` 在 `data_dir/evolution/logs/<id>/`。这些文件是私有运行数据，不应提交。

中断不自动重放。`checking`/`approved` 被显式停止后记为 `failed`；进程崩溃可能保留
最后阶段。`committing`、`committed` 或 `restart_requested` 必须先查看保存的 commit、
分支和重启回执，确认实际结果。新程序启动失败时记录仍保留，不自动回滚或二次启动；
主人可用原先的启动方式运行旧版本。保留 worktree、日志和 release 以便检查，v1 不自动清理。

检查使用临时 Git 仓库、无依赖 Rust fixture、模拟 Moldloft HTTP 响应及审查模型，
不会发送真实聊天消息或重启现有机器人：`cargo test --locked evolution`。
