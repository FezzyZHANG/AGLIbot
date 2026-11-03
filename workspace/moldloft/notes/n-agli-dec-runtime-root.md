---
id: "n-agli-dec-runtime-root"
title: "Deploy: keep the runtime root separate from the source repo; build in a throwaway worktree"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "note:n-agli-dec-no-cloud-split"}]
abstract_zh: "在线 bot 继续跑在独立的运行时根（AGLI_ROOT），不从源码仓库运行：在线 config 本来就和仓库默认值不同，进化闸门会在源码仓库里切分支和提交，Mongo 库要分开，L4 的写权限边界也要落在这条目录线上。去掉常驻的部署 worktree：deploy.ps1 每次在临时 worktree 里构建，共用 target/deploy 缓存，release 只保存 exe。"
abstract_en: "The live bot keeps its own runtime root apart from the source repo: live config diverges from repo defaults, the evolution gate branches and commits in the source repo, the Mongo database must be separate, and L4 write isolation needs a directory boundary. The permanent deploy worktree is dropped: builds use a throwaway worktree, a shared target/deploy cache, and releases hold only the exe."
---
Question (owner, 2026-11-03): do we really need a separate runtime?

Decision: yes for the split between where the bot runs and where git lives; no for the third directory.

Why the runtime root stays (AGLI_ROOT, e.g. `C:\dev\AGLIbot-runtime\<name>`):

1. Live `config/` intentionally differs from the repo's defaults (pinned models, aliases). Running from the checkout would put any branch switch or uncommitted config edit live on the next restart.
2. The evolution gate creates worktrees, branches and commits in the source repo (AGLI_SOURCE) while the bot runs; that was the reason for the 09a8e5d split.
3. The runtime's `.env` points MONGO_DATABASE at its own database, so development and tests never write into live memory.
4. DESIGN section 6 L4 requires that the bot cannot write its own `config/` or `evolution/gate.rs`. That boundary can only be a directory the bot may write versus one it may not; ACLs are not implemented yet, but the directories must be apart first.

A release pointer alone (exe outside the checkout) would avoid Windows exe locking but not points 1 and 4.

What was cut:

- The permanent deploy worktree (`C:\dev\AGLIbot-deploy`) and `deploy_worktree` in `runtime.json`. `ops/deploy.ps1` now adds a detached worktree at `%TEMP%\aglibot-deploy`, builds, and removes it, also on failure.
- One cold `--target-dir` per commit (about 678 MB each). Deploys share `<source>\target\deploy\`, which is outside the runtime root so the bot cannot tamper with build output once L4 lands. A release is only `<runtime>\data\releases\<commit>\aglibot.exe` (about 20 MB). Measured: 69 s for the first build, 21 s afterwards with dependencies cached.

Not done: no multi-runtime features beyond the `<name>` directory level; there is one runtime. Revisit if AGLI moves to an always-on server (see n-agli-dec-no-cloud-split). Operational detail lives in docs/DEPLOY.md.
