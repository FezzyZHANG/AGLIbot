---
id: n-agli-idea-control-handoff
graph_id: default
version: 1
title: Independent master switch and safe evolution handoff
kind: note
state: active
in_graph: true
summary: '超级开关与进化交接的候选验收方案。卡住的模型不应阻断冻结，过期结果和版本不得接管；具体架构、时限及恢复策略待选。


  Candidate control and release contracts, failure cases and open owner choices for
  #87/#26.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-runtime-root
- relation: relates
  target_type: note
  target_id: n-agli-dec-two-layer
- relation: relates
  target_type: ticket
  target_id: 26
- relation: relates
  target_type: ticket
  target_id: 87
---
Review candidate for #87/#26. No execution topology is prescribed. OS isolation #27 remains parked.

## Control contract (#87)

Separate command recognition and admission from persona/model progress. Owner stop/freeze must reach backend control while a persona, judge, network request or delegate is stalled. Acknowledge receipt promptly; distinguish request accepted, cancellation requested and worker termination confirmed. Freeze closes admission before issuing cancellation. Unconfirmed stop blocks replacement writers. Unfreeze restores admission only; it does not replay uncertain work.

Results carry attempt identities. Only the current, non-revoked attempt may update task state or create a local completion notification. External delivery/effects remain reconcilable uncertainties.

Deadlines must advance independently of persona processing. Only observed process-tree exit establishes termination. Record actual or uncertain irreversible effects.

| Requirement | Positive test | Negative/failure test |
|---|---|---|
| Independent stop/freeze | Block persona call; owner command closes admission and cancels active work within a declared bound | Frozen state rejects queued starts; hung cancellation never reports stopped |
| Independent deadlines | Hold model future and delegate fixture; deadlines still fire | Log growth resets stall timer but never extends total budget |
| Current attempt only | Current completion updates once | Duplicate, revoked and pre-stop queued completion cannot publish success |

## Evolution and handoff contract (#26)

Preparation records the active release identity, source commit, data root and candidate base. Reject invalid lineage before work. Review the full diff against that release. Recheck release drift at approval, commit and handoff; a second evolution cannot silently discard an earlier release.

Preflight candidate executable, hash, canonical paths and writable receipt/log destinations before stopping a healthy process. Share runtime identity with normal deployment: release, executable, root, data root, PID plus creation identity, and logs. Distinguish prepared, approved, stopping, starting, running, failed and unknown. Recovery reconciles identity and effects before retry.

| Requirement | Positive test | Negative/failure test |
|---|---|---|
| Lineage | Candidate extends pinned release | Stale base, two unmerged evolutions or post-check release change fails closed |
| Preflight/identity | Synthetic handoff followed by ordinary restart succeeds | Missing/hash-mismatched executable leaves healthy fixture untouched; PID reuse never kills unrelated process |
| Lifecycle | Custom relative/absolute data roots share receipts | Inject failure at each transition; pointer never falsely claims running; restart does not blindly replay unknown handoff |
| Cancellation | Cancel during checks/build blocks publication | Freeze immediately before commit/start retains truthful completed or unknown effects |

Owner decisions still needed: exact latency bounds, whether freeze stops all background services or only task/evolution execution, emergency recovery privileges, and preferred failed-start recovery policy. Preserve writer allowlist, separate owner approval, approved-tree binding and immutable evidence throughout.