---
id: n-agli-q-dsh-search
graph_id: default
version: 1
title: Which web search backend do AGLI's DSH runs use?
kind: question
state: resolved
in_graph: true
summary: '已定（2026-11-01）：DSH 搜索用 DeepSeek 官方，单独一个 key，主人配在 .env。前提是 #46：委托子进程只拿白名单环境变量，DSH
  的 key 用 AGLI_DSH_ 前缀名，只在启动 DSH 时改名传入；隔离运行（#27）前 DSH 仍可直接读 .env 文件。


  Resolved 2026-11-01: DeepSeek official search with a dedicated key in AGLI''s .env,
  on condition of #46 (allowlisted delegate environments; DSH keys under AGLI_DSH_*
  names passed only to the dsh child). Until #27, DSH can still read the .env file.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-deepseek-paid-api
- relation: relates
  target_type: note
  target_id: n-agli-idea-deepseek-app-integration
- relation: relates
  target_type: note
  target_id: n-agli-q-dsh-tokens
- relation: relates
  target_type: ticket
  target_id: 25
---
Found by Astra while implementing #25 (docs/DSH.md, from the installed CLI 0.2.1-rc.5 composition): the model key and the search provider are configured separately. The owner decided on a dedicated Bailian key for the model (n-agli-q-dsh-tokens); search still needs its own backend.

Options:
- DeepSeek official search with a separate DEEPSEEK_API_KEY used only by the agli profile.
- Another search provider DSH supports, if any, configured in the profile patch.
- Fetch-only (no search): the plan step proposes URLs; weaker for open questions.

The key lives in the DSH profile only, never in AGLI's config or anywhere the agents can read.

## Owner answer (2026-11-01)

DeepSeek official search with a dedicated key; the owner will put it in AGLI's .env.

Consequence (Opus): .env is loaded into AGLI's process environment and every delegate child inherits it, so today Codex research runs already see BAILIAN_API_KEY, TELEGRAM_BOT_TOKEN and ONEBOT_TOKEN. Before the DSH keys are added, #46 scrubs delegate environments to an allowlist and passes the DSH keys under prefixed names (AGLI_DSH_DEEPSEEK_API_KEY -> DEEPSEEK_API_KEY, likewise the dedicated Bailian key) to the dsh child only. DSH can still read the .env file itself until the isolated runner exists (#27).

## November 17 scope update

The owner now accepts paid official DeepSeek capabilities as project candidates. This extends the comparison to direct app APIs while preserving the separately attributed DSH search route.
