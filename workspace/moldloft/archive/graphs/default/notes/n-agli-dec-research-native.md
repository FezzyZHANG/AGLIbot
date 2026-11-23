---
id: n-agli-dec-research-native
graph_id: default
version: 1
title: 'Research executor: native, parked until upstream APIs are stable; DSH closed'
kind: decision
state: accepted
in_graph: true
summary: '主人 2026-11-23 决定：研究执行器选 native 而不是 DSH；上游 API 不稳定，先搁置为待办，执行器另作打算。主人认为 DSH
  本身的不稳定是开发的最大阻碍。取代 n-agli-dec-hands 中 DSH 为主要双手的条款。


  Owner decision 2026-11-23: research uses the native worker, not DSH, and stays parked
  until upstream APIs are stable; the executor needs its own plan. Supersedes the
  DSH-as-main-hand clause of n-agli-dec-hands.'
created_at: '2026-11-23T17:28:44Z'
updated_at: '2026-11-23T17:28:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-hands
- relation: relates
  target_type: ticket
  target_id: 116
- relation: relates
  target_type: ticket
  target_id: 25
---
463363c replaced the DSH research path (src/hands/delegate.rs) with the native ResearchWorker (src/work/research.rs). The owner chose native over restoring DSH as an explicit option, but parks research execution because the upstream APIs are unstable, and wants a separate plan for the executor. The owner considers DSH's own instability the biggest obstacle to development, so #25 and the DSH follow-ups #38 and #43 close.

The rest of n-agli-dec-hands stands: Claude Code is the owner's tool, core code goes through Moldloft, and the writer allowlist in root AGENTS.md applies.

Before research is re-enabled: fix R20-R22 from the 463363c review and migrate executor config and credentials explicitly. Never treat `research: dsh` as native silently.