---
id: "n-agli-asm-dsh-apikeyenv"
title: "The agli profile's Bailian provider entry reads its key only from DSH_BAILIAN_API_KEY"
kind: "assumption"
state: "adopted"
relations: [{"type": "assumes", "target": "ticket:46"}, {"type": "assumes", "target": "ticket:25"}, {"type": "relates", "target": "note:n-agli-q-dsh-tokens"}, {"type": "relates", "target": "note:n-agli-q-dsh-search"}]
abstract_zh: "已核实（2026-11-02）：DSH 的模型服务商可在 profile 里用 apiKeyEnv 指定 key 变量。AGLI 专用 DSH home 中的 agli profile 让百炼读 DSH_BAILIAN_API_KEY；实跑成功，去掉该 key 时明确报缺凭据、不回退；搜索只用改名传入的单独 DeepSeek key。"
abstract_en: "Verified 2026-11-02: DSH providers take their key variable from a profile apiKeyEnv. The agli profile in AGLI's own DSH home reads DSH_BAILIAN_API_KEY for Bailian; a live run succeeded, removing the key fails with MISSING_CREDENTIAL and no fallback; search uses only the renamed separate DeepSeek key."
adopted_digest: "fd0298e91e1bac6faf58ca86afba30f2fd8c3138fdd542a57731a8834132b2a3"
---
Claim: in the `agli` profile, the Bailian provider entry `llm-pi-ai.providers.qwen-token-plan-cn`
sets `apiKeyEnv: DSH_BAILIAN_API_KEY`. DSH then authenticates that route only from
`DSH_BAILIAN_API_KEY` in its launch environment, and a missing key fails the run with
`MISSING_CREDENTIAL`; it never falls back to `BAILIAN_API_KEY`, the ambient default
`QWEN_TOKEN_PLAN_CN_API_KEY`, or another provider. Profile text: docs/DSH.md, ticket 46 section.

Scope: only the model key. Search reading `DEEPSEEK_API_KEY` is configuration in stock
`dsh-base` (docs/DSH.md), not part of this premise; see n-agli-q-dsh-search.

What depends on it: #46 forwards `AGLI_DSH_BAILIAN_API_KEY` as `DSH_BAILIAN_API_KEY` to the dsh
child only and keeps AGLI's own `BAILIAN_API_KEY` LocalOnly.

Conditions (each was true in the supporting runs; changing one reopens this note):
- Verified binary: `C:/dev/DSH/resources/runtime/cli/bin/dsh.cmd`, 0.2.1-rc.5, 2026-11-02.
  `AGLI_DSH_BIN` must point there, or the note is re-verified against the actual launcher.
- The profile is based on `headless`, where `credentials` (dsh-credentials-local) is disabled.
  If mounted, a record stored under `DSH_BAILIAN_API_KEY` in `$DSH_HOME/.credentials.yaml`
  would win over the child env, and the "key exists only in the child env" isolation no longer holds.
- `.env` files cannot supply the key: DSH rejects `DSH_*` names in project and home `.env`.

Evidence: n-ev-dsh-apikeyenv-live (fake-key runs A–D and static resolution order).

How to re-verify (no real key needed): with a disposable DSH_HOME holding the `agli` profile,
run `dsh --profile agli --json -` under `env -i` (1) with decoy `BAILIAN_API_KEY` and
`QWEN_TOKEN_PLAN_CN_API_KEY` but no target, expecting `MISSING_CREDENTIAL` naming
`DSH_BAILIAN_API_KEY`; (2) with a fake target, expecting Bailian 401. `--dump-config` must show
the patch and `credentials` disabled. `--json` events carry no provider, so attribution comes
from the error, not usage events.

If refuted (for example an upgrade drops `apiKeyEnv` or reinstates ambient fallback): treat it as
a key-isolation failure, not just a rename. Retarget #46 to the provider's ambient name
(currently `QWEN_TOKEN_PLAN_CN_API_KEY`). The delegate starts from an empty environment, so an
ambient copy is already dropped; keep that name out of `DELEGATE_ENV_ALLOWLIST`, extend the #46
tests so only the dedicated source populates it, and keep a test that a missing
dedicated key fails the DSH run instead of succeeding on another key.

Added 2026-11-02: verification must run with AGLI's own DSH home (#48), not ~/.dsh, whose .credentials.yaml holds the owner's DeepSeek account token. Also check that web_search bills the separate DeepSeek search key (owner uses a dedicated key with separate search billing) and not an account token.

## Verified 2026-11-02 (Opus, owner-authorized)

Profile `agli` built in AGLI's own DSH home (data/dsh-home, #48) from the shipped headless template plus the patch in data/dsh-home/profiles/agli/cordis.patch.yml: llm-pi-ai provider `bailian` (openai-completions, Token Plan base URL, apiKeyEnv: DSH_BAILIAN_API_KEY, model deepseek-v4.1-flash) as agent-default-model, the read-only research patch from docs/DSH.md, workflow-ptc disabled. The composed config (--dump-config) shows credentials and deepseek-account disabled and no credential path/dshHome override.

Live check through AGLI's real env assembly (ignored test hands::delegate::tests::live_dsh_profile_uses_dedicated_keys):
1. With AGLI_DSH_BAILIAN_API_KEY and AGLI_DSH_DEEPSEEK_API_KEY set: the run called web_search and web_fetch and answered correctly; usage recorded (1018 uncached + 2103 cached input, 183 output).
2. Without AGLI_DSH_BAILIAN_API_KEY: DSH failed with MISSING_CREDENTIAL for provider route "bailian" naming DSH_BAILIAN_API_KEY; no fallback to DEEPSEEK_API_KEY or another provider.
3. Search key: the child's only DEEPSEEK_API_KEY is the renamed AGLI_DSH_DEEPSEEK_API_KEY and the credential/account plugins are disabled, so web_search used the owner's separately billed search key.

Gap: AGLI records the model as `dsh:profile:agli`; the provider/model from the events is not yet stored.