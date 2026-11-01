---
id: "n-agli-asm-dsh-apikeyenv"
title: "The agli profile's Bailian provider entry reads its key only from DSH_BAILIAN_API_KEY"
kind: "assumption"
state: "proposed"
relations: [{"type": "assumes", "target": "ticket:46"}, {"type": "assumes", "target": "ticket:25"}, {"type": "relates", "target": "note:n-agli-q-dsh-tokens"}, {"type": "relates", "target": "note:n-agli-q-dsh-search"}]
abstract_zh: "前提：agli profile 里百炼 provider 条目（qwen-token-plan-cn）设 apiKeyEnv: DSH_BAILIAN_API_KEY 后，DSH 只从子进程环境的这个名字取 key，缺键即失败而不回退。已由 2026-11-02 对 0.2.1-rc.5 的假 key 实测支持（n-ev-dsh-apikeyenv-live）。前提是 agli 基于 headless、不挂 credentials 服务；DSH 升级或 profile 基座变化时重验。"
abstract_en: "Premise: with apiKeyEnv: DSH_BAILIAN_API_KEY on the agli profile's Bailian provider entry, DSH reads the key only from that child-env name and fails rather than falling back. Supported by 2026-11-02 fake-key runs on 0.2.1-rc.5, assuming a headless base with no credentials service; re-verify on DSH upgrades or base changes."
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