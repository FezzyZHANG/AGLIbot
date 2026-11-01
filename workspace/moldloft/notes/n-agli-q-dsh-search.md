---
id: "n-agli-q-dsh-search"
title: "Which web search backend do AGLI's DSH runs use?"
kind: "question"
state: "open"
relations: [{"type": "relates", "target": "ticket:25"}, {"type": "relates", "target": "note:n-agli-q-dsh-tokens"}]
abstract_zh: "提醒主人：DSH 自带 headless 的 web_search 走 deepseek-official，需要 DEEPSEEK_API_KEY，百炼的 key 管不到搜索。要定：AGLI 的 DSH 搜索用哪个后端、用谁的 key、是否也和主人自己的分开。"
abstract_en: "Owner reminder: DSH's stock headless web_search uses deepseek-official and needs DEEPSEEK_API_KEY; the dedicated Bailian key does not cover search. Decide the backend and key for AGLI's DSH searches, and whether it is separate from the owner's own."
---
Found by Astra while implementing #25 (docs/DSH.md, from the installed CLI 0.2.1-rc.5 composition): the model key and the search provider are configured separately. The owner decided on a dedicated Bailian key for the model (n-agli-q-dsh-tokens); search still needs its own backend.

Options:
- DeepSeek official search with a separate DEEPSEEK_API_KEY used only by the agli profile.
- Another search provider DSH supports, if any, configured in the profile patch.
- Fetch-only (no search): the plan step proposes URLs; weaker for open questions.

The key lives in the DSH profile only, never in AGLI's config or anywhere the agents can read.