# DSH hand: ticket 25 owner handoff

The code is opt-in. Research defaults to Codex; code tasks still use Codex.
Set `AGLI_DSH_BIN` in the bot's environment to
`C:/dev/DSH/resources/runtime/cli/bin/dsh.cmd` (or an owner-provided isolated
launcher). Without it the hand resolves `dsh.cmd` on Windows, `dsh` elsewhere.
The launcher must preserve stdin, stdout JSONL, arguments, cwd and cancellation.
Managing a remote/WSL process from a Windows shim needs runner-specific cleanup;
that deployment is ticket 27, not implemented here.

Merge these lines into the existing `hands` mapping in `config/runtime.yaml`:

```yaml
hands:
  research: dsh
  dsh_profile: agli
  code: codex
```

Keep the existing `codex_model` and `timeout_min`. The owner provisions the profile,
Bailian model/key, search backend and isolated runner. No daily cap is enforced.
The stock headless web search provider is `deepseek-official`, using
`DEEPSEEK_API_KEY`; a Bailian model key alone does not configure that search service.

## Delegate environment and dedicated keys (ticket 46)

Both hands start with an empty environment. The single `DELEGATE_ENV_ALLOWLIST`
in `src/hands/delegate.rs` restores runtime paths, Windows/npm startup variables,
temporary directories, user/application directories, `CODEX_HOME`, `DSH_HOME`,
and HTTP/HTTPS/ALL/NO proxy variables (including lowercase spellings).
Windows variable matching is case-insensitive. Codex retains its existing login
under `CODEX_HOME` or `%USERPROFILE%\.codex`; it receives no AGLI/provider keys.
Variables not explicitly allowed, including `NODE_OPTIONS`, are discarded.

Secret metadata lives in `ns::secrets::SecretRegistry`: variable name,
`ProcessingPolicy`, and optional child-side rename. `allows_executor` treats
`LocalOnly` as never leaving AGLI's process, even for a local CLI. The existing
model-policy semantics are unchanged. The runtime allowlist cannot override a
registered policy. `Config::load` registers every `providers.*.api_key_env` from
`models.yaml` as `LocalOnly`, along with configured owner-ID, Telegram-proxy,
OneBot-token/URL, and Mongo-URL environment names. No values enter the registry.
Delegates take a process-registry snapshot; subsequent config loads only add
local restrictions, including overriding a conflicting delegate grant.

Unregistered names containing KEY/TOKEN/SECRET/PASSWORD/COOKIE/CREDENTIAL,
OWNER_* names, and connection-string names are denied case-insensitively even
if accidentally added to the runtime allowlist. Connection-string matching is
conservative: names ending in `_URL`, `_URI`, `_DSN`, bare `DSN`, or containing
`CONNECTION_STRING`/`CONNECTIONSTRING`. Proxy URLs with userinfo are also denied;
ordinary proxy URLs remain allowed. `TELEGRAM_PROXY` itself is always local.

The owner supplies two dedicated credentials in AGLI's `.env` or launch
environment (see the empty placeholders in `.env.example`):

| AGLI parent variable | DSH child variable | Purpose |
| --- | --- | --- |
| `AGLI_DSH_DEEPSEEK_API_KEY` | `DEEPSEEK_API_KEY` | DeepSeek web search |
| `AGLI_DSH_BAILIAN_API_KEY` | `DSH_BAILIAN_API_KEY` | Bailian model provider |

Neither prefixed name is forwarded. Ambient `BAILIAN_API_KEY`,
`DEEPSEEK_API_KEY`, `DSH_BAILIAN_API_KEY`, `TELEGRAM_BOT_TOKEN`, and `ONEBOT_TOKEN`
are discarded; only the two dedicated sources can populate the DSH targets.
Missing or empty dedicated keys produce a hint naming the source variable,
without a value, and DSH still starts and reports its own credential errors.
Codex does not require either key.

Installed CLI 0.2.1-rc.5's `dsh-llm-pi-ai/lib/index.js` makes `apiKeyEnv`
configurable per provider and resolves it through the credential service or
launch environment. In the owner's `agli` profile, set the existing Bailian
provider entry under `llm-pi-ai.providers` to:

```yaml
apiKeyEnv: DSH_BAILIAN_API_KEY
```

Keep its existing model IDs, protocol and base URL. The stock
`dsh-base/cordis.patch.yml` already sets search's `apiKeyEnv: DEEPSEEK_API_KEY`.
These facts were checked read-only in the installed `app.asar`; no owner profile
or credentials were changed. The profile line and dedicated keys must be
provisioned by the owner before relying on DSH. Environment scrubbing does not
restrict file reads or remove credentials stored in a CLI home; the isolated
runner requirement below still applies.

Ticket 46 tests launch this crate's harmless environment-printing child with a
synthetic parent map: allowed variables survive, bot/future secrets are absent,
and only DSH receives the renamed dedicated keys. Missing/empty keys and Windows
case-insensitive names are covered without global environment mutation. Pure
tests cover policy decisions, environment assembly, accidental allowlist entries,
credential-bearing proxies, new provider names, and config-load registration.
Windows validation after the ns refactor: `cargo test --locked` passed
(103 passed, 7 ignored), as did build, clippy with warnings denied, and
`git diff --check`. The approved existing `live_codex_round_trip` test ran once
and passed during the first pass; it was not repeated after the policy refactor.

### Possible future owner-managed ACL representation

This is a proposal for `config/acl.yaml`, not a currently supported config field.
The implementation uses code defaults; no `config/` file was changed.

```yaml
# 将来迁入 ACL 的边界配置；当前版本尚不读取此字段。
# local_only 表示不得离开 AGLI 进程，包含本机启动的 CLI。
secret_variables:
  - {name: TELEGRAM_BOT_TOKEN, policy: &local {kind: local_only}}
  - {name: TELEGRAM_PROXY, policy: *local}
  - {name: ONEBOT_TOKEN, policy: *local}
  - {name: BAILIAN_API_KEY, policy: *local}
  - {name: OWNER_TELEGRAM_ID, policy: *local}
  - {name: OWNER_QQ_ID, policy: *local}
  - {name: MONGO_URL, policy: *local}
  # DSH 独享的专用密钥；Codex 无接收权限。
  - name: AGLI_DSH_DEEPSEEK_API_KEY
    policy: {kind: providers, names: [dsh]}
    child_name: DEEPSEEK_API_KEY
  - name: AGLI_DSH_BAILIAN_API_KEY
    policy: {kind: providers, names: [dsh]}
    child_name: DSH_BAILIAN_API_KEY
# 仍须在加载时自动注册 models.yaml 的 providers.*.api_key_env 为 local_only，
# 以及配置中的主人 ID、代理、渠道令牌和数据库连接变量名；无需手工枚举新 provider。
# 未注册的疑似密钥、OWNER_* 和连接串变量始终默认拒绝；运行时白名单不能放行。
```

## Proposed research profile patch (not installed)

Base the `agli` profile on the shipped `headless` profile. This proposal targets
CLI 0.2.1-rc.5's exact plugin composition. It leaves the model-facing tools
`web_search`, `web_fetch`, and `read` registered and disables the other tool
providers; native mode avoids exposing the PTC `run_code` transport.
The filesystem tool plugin's `apply()` conditionally registers read/write/edit,
so these switches remove capabilities in DSH code, not through a prompt.

```yaml
- id: tools
  config: {mode: native}
- id: tool-web
  config: {search: true, fetch: true, searchTimeoutMs: 60000}
- id: tool-fs
  config: {read: true, readImage: false, write: false, edit: false}
- id: sandbox-policy
  config:
    mode: read-only
    workspaceRoot: !!js process.cwd()
- id: permission
  config:
    defaultPreset: read-only
    presets:
      read-only: {sandbox: read-only, approval: never}
- id: approval
  config: {policy: never}
- {id: tool-plugin-manager, disabled: true}
- {id: plugin-manager, disabled: true}
- {id: tool-working-directory, disabled: true}
- {id: tool-bash, disabled: true}
- {id: tool-pwsh, disabled: true}
- {id: tool-jobs, disabled: true}
- {id: tool-fs-search, disabled: true}
- {id: tool-skill, disabled: true}
- {id: plan-mode, disabled: true}
- {id: subagent, disabled: true}
- {id: subagent-spawn-in-process, disabled: true}
- {id: subagent-fork-in-process, disabled: true}
- {id: tool-subagent-control, disabled: true}
- {id: tool-subagent-list-agents, disabled: true}
- {id: tool-subagent, disabled: true}
- {id: tool-subagent-fork, disabled: true}
- {id: tool-workflow, disabled: true}
- {id: tool-todo, disabled: true}
- {id: tool-goal, disabled: true}
- {id: mcp-resources, disabled: true}
```

`read-only` restricts writes, **not read paths**: `dsh-fs-sandbox/lib/index.js`
explicitly passes reads through. The `read` tool can read the task directory,
but the owner must provide OS isolation to prevent reads elsewhere, including
credentials. `approval: never` disables interactive escalation, not normal
allowed reads. Do not treat this YAML as the isolated runner itself.
Recheck the final composition after owner/home patches or DSH upgrades; those
layers can add tools or replace these settings. No live allowlist probe was run.

The proposal comes from successful `--profile headless --dump-config` and
`--profile headless --dump-config-schema` exports against a disposable `DSH_HOME`.
Both exports also succeeded with this proposal passed as `--patch`; schema
collection reported `complete: true` and no diagnostics. This checks composition
and schema availability, not a mounted profile or a live tool-denial probe.
Schema definitions: `config24` (sandbox policy), `config27` (approval), `config28`
(permission presets), `config33` (filesystem tools), `config59` (web tools),
`config60` (tool presentation), and `patchList`/`patch` (id-targeted overlays).
Patch `config` replaces the entire configuration, rather than merging fields.
Both export commands call `prepareProfile()` and rewrite the generated
`cordis.yml`; the initial default-home attempts were blocked by the filesystem
sandbox, so subsequent inspection used a temporary home. No original profile
was changed and no live DSH task was run.

Source paths are under
`C:/dev/DSH/resources/app.asar/dsh/node_modules/@deepseek-ai/`:

- `dsh-headless/lib/json-stream-BA-F3lfb.js`: `projectJsonRun`, `addUsage`, event
  bounds and truncation; `dsh-headless/lib/index.js`: error event and session resume.
- `dsh-tool-fs/lib/index.js`: `Config`/`apply` (conditional tool registration).
- `dsh-tool-web/lib/index.js`: `web_search`/`web_fetch` schemas.
- `dsh-fs-sandbox/lib/index.js`: reads are unrestricted by the filesystem sandbox.
- `dsh/lib/profile-boot-C14i-8IN.js`: `prepareProfile` writes even during dumps.

## Runtime behavior and verification

Each invocation appends raw stdout lines to `dsh-events.jsonl`, parsed tool calls
and URL/path targets to `dsh-tool-calls.jsonl`, and a short summary to `log.md`.
Search queries remain in the tool input; a search call has no fetched URL target.
Malformed/unknown events are ignored by the projection but retained in raw audit.
DSH itself bounds most event payloads, so missing/truncated targets are explicitly
preserved as such; AGLI cannot reconstruct bytes the CLI omitted. Only `final.text`
is accepted as the answer. Session IDs are captured and checked on continuation.

Each reported `status/step_end.usage` contributes once to one `ModelCallRecord`
per invocation, including failure/timeout/cancellation. Retried model attempts
are already included by DSH. Missing usage is logged, and missing steps make a
recorded sum partial. Cost is zero/unknown; `model_id` identifies the profile
because this event schema does not identify the underlying model. Neither a
budget gate nor a guessed price is introduced.

The research hand is read-only. Its final answer must be a JSON object with
`summary`, `report`, `claims` (YAML text), and `sources` (`url`, `content`). AGLI
saves those into fixed paths for the existing rule checks and judge. This is a
deliberate adaptation of the writing Codex gather contract; invalid JSON fails
the gather and remains available in raw audit. Followups, bounded gather/review
iterations and one fresh-session fallback on resume failure reuse ticket 33.

Windows uses a kill-on-close Job Object. The `.cmd` launcher starts suspended,
joins the job, then resumes, preventing child creation before containment.
Unix delegates use a dedicated process group; that branch was not exercised on
this Windows machine. No OS isolation or remote-runner containment is implied.

Deterministic tests cover the installed event schema, malformed/missing events,
profile/command construction, stdin transport, usage and audit on success and
failure, session identity, missing final/error events, research artifact
materialization, and existing followup boundaries. Two Windows tests launch a
harmless `.cmd`/child/grandchild tree and verify actual process exit on timeout
and `JoinHandle::abort` (the `/stop` and `/freeze` path); timeout also covers a
blocked stdin write. No added tests require real models or networks.

`windows-sys` and `libc` become direct target-specific dependencies for lifecycle
control; both versions were already present in `Cargo.lock`.

Ticket 25 Windows validation: `cargo test --locked` reported
`93 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out`.
`cargo build --locked`, `cargo clippy --locked --all-targets -- -D warnings`, and
`git diff --check` passed. The checkout's test suite is larger than the ticket's
older 48-passed/4-ignored baseline; no live/ignored test was enabled.
