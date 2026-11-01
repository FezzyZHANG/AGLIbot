---
id: "n-agli-q-dsh-tokens"
title: "Which token source and budget does DSH use for AGLI's runs?"
kind: "question"
state: "resolved"
relations: [{"type": "relates", "target": "ticket:25"}]
abstract_zh: "已定（2026-11-01）：AGLI 的 DSH 运行用百炼，单独一个 key（与主人自己用的 DSH 和 AGLI 自身的百炼 key 分开），日上限先定 ¥20。key 只放在 DSH profile 里；上限要在服务商或 DSH 侧强制执行。待 #25 核实：百炼能否按 key 设日限额、--json 事件是否带用量。Codex 额度问题留给 #39。"
abstract_en: "Resolved 2026-11-01: AGLI's DSH runs use Bailian with a dedicated key (separate from the owner's DSH use and AGLI's own Bailian key), hard cap CNY 20/day to start. Key only in the DSH profile; cap enforced provider- or DSH-side. To verify under #25: per-key daily limits on Bailian, usage in --json events. Codex quota stays with #39."
---
DSH is now AGLI's main hand (#25), so most of his spending will go through DSH rather than AGLI's own model calls.

To decide:
- Provider and key for AGLI's DSH profile (DeepSeek official, Bailian, or another).
- A key separate from the owner's own DSH profiles, so usage is attributable and can be revoked without touching the owner's work.
- Hard caps per run and per day, enforced on the provider or DSH side, not only by AGLI.
- Where the key lives: in the DSH profile, never in AGLI's config or anything the agents can read (DESIGN section 6, other guardrails).
- How AGLI reads usage: from `dsh --json` run events, if they report tokens.
- If dsh-subagent-codex is adopted (#39 evaluation), whose Codex login and quota it uses.

## Owner answer (2026-11-01)

- Provider and key: Bailian, with a dedicated key used only by AGLI's DSH profile, separate from the owner's own DSH use and from AGLI's own `BAILIAN_API_KEY`.
- Hard cap: CNY 20 per day to start.
- Unchanged requirements: the key lives in the DSH profile, never in AGLI's config or anywhere the agents can read; the cap is enforced on the provider or DSH side, with AGLI's own accounting from `dsh --json` usage events as a second line, not the only one.

Still to verify under #25: whether the Bailian console can enforce a per-key daily limit, or whether DSH must enforce it; and whether DSH's --json events report tokens. The Codex quota question for dsh-subagent-codex stays with the #39 evaluation.
