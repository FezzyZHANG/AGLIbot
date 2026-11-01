---
id: "n-agli-asm-dsh-apikeyenv"
title: "DSH model providers read their key from a profile-configurable apiKeyEnv"
kind: "assumption"
state: "proposed"
relations: [{"type": "assumes", "target": "ticket:46"}, {"type": "assumes", "target": "ticket:25"}, {"type": "relates", "target": "note:n-agli-q-dsh-tokens"}, {"type": "relates", "target": "note:n-agli-q-dsh-search"}]
abstract_zh: "待核实：Astra 静态查看已安装的 DSH CLI 0.2.1-rc.5 后认为，模型服务商的 key 变量名可在 profile 里用 apiKeyEnv 配置。#46 据此让 AGLI 把 AGLI_DSH_BAILIAN_API_KEY 改名为 DSH_BAILIAN_API_KEY 只传给 DSH，agli profile 的百炼服务商写 apiKeyEnv: DSH_BAILIAN_API_KEY。尚未实际运行验证。"
abstract_en: "To verify: from static inspection of the installed DSH CLI 0.2.1-rc.5, Astra concluded that a model provider's key variable is configurable per profile via apiKeyEnv. #46 relies on it: AGLI renames AGLI_DSH_BAILIAN_API_KEY to DSH_BAILIAN_API_KEY for the dsh child only, and the agli profile's Bailian provider sets apiKeyEnv: DSH_BAILIAN_API_KEY. Not exercised live."
---
Source: Astra's #46 implementation report (2026-11-01), static reading of the installed CLI and a disposable-DSH_HOME config dump; no live DSH run, no key involved.

What depends on it:
- #46 maps AGLI_DSH_BAILIAN_API_KEY (in AGLI's .env, LocalOnly to AGLI except for the dsh executor) to DSH_BAILIAN_API_KEY in the dsh child's environment.
- The proposed agli profile (docs/DSH.md) sets the Bailian provider's `apiKeyEnv: DSH_BAILIAN_API_KEY`, so it never reads AGLI's own BAILIAN_API_KEY.
- Search is separate: DEEPSEEK_API_KEY for deepseek-official web_search (n-agli-q-dsh-search) is assumed to be read under that fixed name.

How to verify (before enabling hands.research: dsh):
1. `dsh --profile agli --dump-config` (disposable DSH_HOME if dumps write) shows the Bailian provider with apiKeyEnv: DSH_BAILIAN_API_KEY and the model in use is that provider.
2. A minimal live headless run with only DSH_BAILIAN_API_KEY and DEEPSEEK_API_KEY in the child env (AGLI's BAILIAN_API_KEY absent) succeeds; with DSH_BAILIAN_API_KEY removed it fails with a missing-key error, not by silently falling back to another variable or provider.
3. The --json usage events attribute the run to the Bailian provider.

If refuted: fall back to the provider's default variable name and make the rename in #46 target it, keeping the key Providers{dsh}-only.