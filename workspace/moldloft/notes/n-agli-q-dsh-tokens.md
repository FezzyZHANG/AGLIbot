---
id: "n-agli-q-dsh-tokens"
title: "Which token source and budget does DSH use for AGLI's runs?"
kind: "question"
state: "open"
relations: [{"type": "relates", "target": "ticket:25"}]
abstract_zh: "提醒主人：DSH 用自己的模型配置和密钥，和 AGLI 的模型配置是两套。要定下 AGLI 的 DSH 运行用哪个服务商和密钥、是否与主人自己用 DSH 分开、预算硬上限多少、用量怎么读。接入 dsh-subagent-codex 后还会用到 Codex 的额度。"
abstract_en: "Owner reminder: DSH has its own model configuration and keys, separate from AGLI's. Decide which provider and key AGLI's DSH runs use, whether they are separate from the owner's own DSH use, the hard budget, and how usage is read. dsh-subagent-codex would also draw on Codex quota."
---
DSH is now AGLI's main hand (#25), so most of his spending will go through DSH rather than AGLI's own model calls.

To decide:
- Provider and key for AGLI's DSH profile (DeepSeek official, Bailian, or another).
- A key separate from the owner's own DSH profiles, so usage is attributable and can be revoked without touching the owner's work.
- Hard caps per run and per day, enforced on the provider or DSH side, not only by AGLI.
- Where the key lives: in the DSH profile, never in AGLI's config or anything the agents can read (DESIGN section 6, other guardrails).
- How AGLI reads usage: from `dsh --json` run events, if they report tokens.
- If dsh-subagent-codex is adopted (#39 evaluation), whose Codex login and quota it uses.
