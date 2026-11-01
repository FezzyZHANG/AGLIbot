---
id: "n-ev-dsh-agent-teams-idempotency"
title: "Attempt generations and mailbox receipts are useful at higher concurrency"
kind: "evidence"
state: "recorded"
relations: [{"type": "derived_from", "target": "note:n-src-dsh-agent-teams"}, {"type": "supports", "target": "ticket:40"}, {"type": "supports", "target": "ticket:41"}, {"type": "relates", "target": "ticket:29"}, {"type": "relates", "target": "ticket:35"}, {"type": "relates", "target": "ticket:33"}, {"type": "relates", "target": "ticket:37"}, {"type": "relates", "target": "ticket:25"}]
abstract_zh: "源码显示插件通过 attempt、接管停止和邮箱回执处理重试与重复事件；这些机制不能证明任意外部动作恰好执行一次。AGLI 当前低并发是主要差别：幂等性增强低优先级，出现重复执行或实际增加并发后再推进；防御性审查收益和优先级均低。"
abstract_en: "Observed attempt-generation and mailbox-receipt mechanisms inform low-priority AGLI idempotency work. Concurrency is the main workload difference; no exactly-once external effects or measured AGLI benefit are established."
---
Observed at the pinned revision in n-src-dsh-agent-teams:

1. state.ts beginTaskAttempt increments the attempt generation and creates a fresh attemptId. invalidateTaskAttempt revokes it before handoff; tools.ts waits for the previous activation to stop before allowing the next dispatch. Member updates with a superseded current attempt ID are rejected, and terminal updates cannot replace the terminal result.
2. scheduler.ts retains an observed parked attempt across idle boundaries, while a cold-recovered open task obtains a new attempt. This avoids interpreting every idle or missing handle as a reason to start the work again.
3. state.ts separates deliveryClaimedAt, deliveredAt and readAt. scheduler.ts records delivery only after the host accepts the message and releases failed delivery claims for retry. Acceptance by the host is explicitly not evidence that the model consumed the input.
4. The mutation queue is process-local. These mechanisms do not establish exactly-once side effects in arbitrary external systems.

AGLI applicability (engineering inference): use a small per-task attempt identifier and terminal-result receipt to distinguish duplicate/current/obsolete completions; when #32/#33 introduce follow-ups and continuable sessions, use stable input IDs and a durable consumption checkpoint. Reconcile uncertain external effects before retrying rather than treating a new attempt as proof that repeating an action is safe.

Priority judgment supplied by the owner on 2026-11-01: concurrency is the main difference. AGLI currently runs one delegated job and planned #37 defaults max_jobs to 1. Keep new idempotency enhancements low priority; revisit after observed duplicate work or when overlapping attempts, automatic retries or multiple workers become real usage. Defensive review gates, artifact-review hash binding and extra review loops have low expected benefit and priority here; they are outside this batch. No measured AGLI race or production failure is claimed.

Since #25 (2026-11-01) DSH is AGLI's primary hand, and this plugin runs inside DSH. Installing it is not proposed: the mechanisms above are the useful part, and its multi-member scheduling targets the concurrency AGLI does not yet have.

Source anchors are recorded in n-src-dsh-agent-teams and in the linked tickets.