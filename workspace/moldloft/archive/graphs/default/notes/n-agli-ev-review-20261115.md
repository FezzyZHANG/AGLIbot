---
id: n-agli-ev-review-20261115
graph_id: default
version: 1
title: What the audit and re-acceptance checks established
kind: evidence
state: recorded
in_graph: true
summary: '区分隔离复现、源码追踪、历史运行记录与待补实测。旧研究日志证实末轮要求修改后仍进入 Done。


  Evidence by strength, including a historical final Revise followed by Done.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-src-review-20261115
- relation: relates
  target_type: ticket
  target_id: 25
- relation: relates
  target_type: ticket
  target_id: 26
- relation: relates
  target_type: ticket
  target_id: 28
- relation: relates
  target_type: ticket
  target_id: 5
- relation: relates
  target_type: ticket
  target_id: 84
- relation: relates
  target_type: ticket
  target_id: 85
- relation: relates
  target_type: ticket
  target_id: 86
- relation: relates
  target_type: ticket
  target_id: 87
- relation: relates
  target_type: ticket
  target_id: 92
- relation: relates
  target_type: ticket
  target_id: 98
- relation: relates
  target_type: ticket
  target_id: 99
---
The November 15 repeat run of cargo test --locked --offline completed with 374 passed, zero failed and 17 ignored. Loopback test fixtures used a local connection. Ignored real-service checks were excluded. Earlier restricted-environment failures are superseded. Test totals cover the existing suite only.

Isolated production-module checks confirmed A01 (free-text growth canary reaches audience-independent L1), A03 (two allocations and one overwritten review), the stale-record half of A05 (Disabled/Origin restored to Active/Any), and A22 (current shipped routes cannot satisfy LocalOnly for consolidate or pinned chat). The other A05 dispatch path and A02 are traced from production callers.

The real start script with mock process/hash boundaries reproduced A12 and A14 without touching a running process. A04 and A16 follow from version and path resolution code. A06 is an event-loop control-path finding. Child process call timeouts still exist. M01 was observed with the production renderer and a temporary store. M02 was checked at middleware level only.

A13 now also has a historical receipt in work/tasks/20261102-1757-研究-arxiv261032813-Harnes/log.md. The task logged two DSH completions in one session and a final request for revision immediately before Done. Its tool audit contains 3 search and 35 fetch entries. This corroborates the state mismatch and historical research integration. Tool use does not identify every tool the worker could have called.

Other A07–A21 findings are traced to production paths and interleavings as individually indicated in REVIEW.md. Ticket acceptance tests specify future successful behavior. Current regression fixtures and a prior Done label alone do not certify a repair.