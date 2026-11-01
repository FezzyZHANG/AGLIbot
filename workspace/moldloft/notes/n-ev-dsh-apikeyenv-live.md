---
id: "n-ev-dsh-apikeyenv-live"
title: "Live fake-key runs: DSH reads the Bailian key only from DSH_BAILIAN_API_KEY"
kind: "evidence"
state: "recorded"
relations: [{"type": "supports", "target": "note:n-agli-asm-dsh-apikeyenv"}, {"type": "relates", "target": "ticket:46"}, {"type": "relates", "target": "ticket:25"}]
abstract_zh: "2026-11-02 在跑 bot 的机器上用 C:/dev/DSH 的 CLI 0.2.1-rc.5、一次性 DSH_HOME 和假 key 实测：provider 条目设 apiKeyEnv 后只读该名；缺键报 MISSING_CREDENTIAL，不回退到 BAILIAN_API_KEY 或默认名 QWEN_TOKEN_PLAN_CN_API_KEY；假 key 被百炼以 401 拒收；.env 不能设 DSH_ 前缀变量。未用真 key。"
abstract_en: "On 2026-11-02, fake-key runs of the bot machine's DSH CLI 0.2.1-rc.5 in a disposable DSH_HOME showed apiKeyEnv on the provider entry is the only key source: a missing key fails with MISSING_CREDENTIAL instead of falling back, a fake key reaches Bailian and gets 401, and .env files cannot set DSH_* names. No real key used."
---
Provenance: Opus via the owner's Claude Code, 2026-11-02, on the machine that runs the bot.
Binary `C:/dev/DSH/resources/runtime/cli/bin/dsh.cmd`, `--version` 0.2.1-rc.5, the path docs/DSH.md
names for `AGLI_DSH_BIN`. Static reads are from that install's `resources/app.asar`. A D:/DSH
0.2.1-rc.3 on another machine is a different install and was not tested.

Setup: disposable DSH_HOME holding one `agli` profile copied from the shipped `headless` profile,
with this patch layer (the real `~/.dsh` was not touched):

```yaml
- id: llm-pi-ai
  config: {providers: {qwen-token-plan-cn: {apiKeyEnv: DSH_BAILIAN_API_KEY}}}
- id: agent-default-model
  config: {provider: qwen-token-plan-cn, model: deepseek-v4-flash}
```

`qwen-token-plan-cn` is pi-ai's built-in provider whose base URL equals AGLI's
`BAILIAN_BASE_URL_OAI`. `--dump-config` showed both patches applied and `credentials`
(dsh-credentials-local) disabled, as in stock `headless`. Each run used `env -i` with only
OS/path variables plus the variables named below, then `dsh --profile agli --json -` with prompt "hi".

| Run | Child env / files | Result |
| --- | --- | --- |
| A | `BAILIAN_API_KEY` and `QWEN_TOKEN_PLAN_CN_API_KEY` set to decoys; target absent | `MISSING_CREDENTIAL`, message names `DSH_BAILIAN_API_KEY`; no request made |
| B | `DSH_BAILIAN_API_KEY=sk-fake-…` | Bailian 401 `invalid_api_key` (Model Studio error page) |
| C | target only in `$DSH_HOME/.env` | boot refused: `.env` may not set `DSH_BAILIAN_API_KEY` |
| D | target only in `<cwd>/.env` | same refusal |

Static facts from `dsh-llm-pi-ai/lib/index.js` and neighbours, consistent with the runs:
- `apiKeyEnv` is a field of one provider entry under `llm-pi-ai.providers`, schema role
  `credential-ref`; it is not a profile-wide switch.
- With `apiKeyEnv` set, `resolveApiKey` reads only that name. It asks the credentials service
  when one is mounted, otherwise the launch environment; if empty it throws `MISSING_CREDENTIAL`.
  pi-ai's ambient discovery runs only for providers without `apiKeyEnv`.
- The ambient default for `qwen-token-plan-cn` is `QWEN_TOKEN_PLAN_CN_API_KEY`, not `BAILIAN_API_KEY`.
- dsh-credentials-local, when mounted, resolves stored file record > process env > project
  `.env` > `$DSH_HOME/.env`. A stored record would beat the child env. `$DSH_HOME/.credentials.yaml`
  exists on the bot machine; stock `headless` does not mount the service, so it is not read.
- dsh-app-boot rejects bootstrap-only names (including the `DSH_` prefix) in either `.env`
  layer, which is why runs C/D refused. Non-`DSH_` names such as `DEEPSEEK_API_KEY` are
  accepted from those files.
- `--json` events carry no provider or model field; run B's provider is identified by the
  Bailian endpoint's error, not by usage attribution.

Not covered: a successful call with a real key, `hands.research` through AGLI's own
delegate (these runs used `env -i`, not `delegate_environment`), and web search.
