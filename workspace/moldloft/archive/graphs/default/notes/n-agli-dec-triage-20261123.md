---
id: n-agli-dec-triage-20261123
graph_id: default
version: 1
title: Feature triage after the Gemini refactor (owner sign-off 2026-11-23)
kind: decision
state: accepted
in_graph: true
summary: '主人 2026-11-23 签字：恢复 lessons + Widen、commitments、学习型群观察；周成长和探针等 #84；知识卡排在研究执行器之后；reciprocity
  和人物卡导出删除；flow、向量召回、watchlist 记待办；进化闸门退役；研究执行器选 native 并搁置。


  Owner sign-off 2026-11-23 on the features Gemini deleted in 0fe9648 and replaced
  in 463363c: restore lessons+Widen, commitments and learned group observations; growth
  and probes wait for #84; knowledge cards follow the research executor; delete reciprocity
  and person-card export; park flow, embeddings and watchlist; retire the gate; research
  goes native and is parked.'
created_at: '2026-11-23T17:28:44Z'
updated_at: '2026-11-23T17:28:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-gate-retired
- relation: relates
  target_type: note
  target_id: n-agli-dec-research-native
- relation: relates
  target_type: ticket
  target_id: 104
- relation: relates
  target_type: ticket
  target_id: 112
- relation: relates
  target_type: ticket
  target_id: 113
- relation: relates
  target_type: ticket
  target_id: 114
- relation: relates
  target_type: ticket
  target_id: 115
- relation: relates
  target_type: ticket
  target_id: 116
- relation: relates
  target_type: ticket
  target_id: 117
- relation: relates
  target_type: ticket
  target_id: 28
- relation: relates
  target_type: ticket
  target_id: 5
- relation: relates
  target_type: ticket
  target_id: 84
---
Source: docs/reviews/2026-11-23/FEATURE-TRIAGE.md (Opus and Astra assessed each feature independently, then merged). Features without an owner change follow the merged recommendation.

| Feature | Disposition |
|---|---|
| Weekly growth (#5) | Restore simplified: bounded numeric/style deltas (about ±0.02 a week, ±0.15 in total) with source IDs and versions, revertible; no free text into L1; the narrative stays private. Waits for #84. |
| Persona probes (#28) | Restore with #5; run when the persona version changes; judge boundary questions separately; bind approvals to that run's answers. |
| Knowledge cards | New feature after the research executor, not the old clusterer. |
| Reciprocity | Delete the scorer. Evidence-backed help or broken promises may go into person facts, with no scoring call. |
| Person-card export | Delete; build a read-only page later if needed. |
| Learned group observations | Restore (owner changed the 'park' recommendation) after confirming scope and design gaps. |
| Archive and Episode.sources | Keep sources through migration (release blocker); model summaries can wait until a backlog appears. |
| Episode embeddings | TODO: collect missed-recall cases first. |
| Watchlist | Park. A later minimal version: explicit subscriptions with one reminder when due, no automatic paid research. |
| Commitments | Restore explicit promises as a solo-decision input. |
| Lessons and Widen | Restore together, including the first chat reader. |
| Evolution pipeline | Retired: n-agli-dec-gate-retired. |
| flow / flow_metrics | TODO (owner changed the 'keep deleted' recommendation). |
| Research executor | Native chosen, parked: n-agli-dec-research-native. |

Order: release blockers first; then lessons+Widen and commitments (which goes first is still the owner's call); then learned group observations. Growth and probes start after #84. Knowledge cards follow the executor.

Data: until each feature is settled, stop writing the old collections but do not delete them (lessons, relations, groups, embeddings, agenda:commitments, life:watchlist, persona_probe_*, knowledge:checkpoint). Do not reconnect the old drift.yaml, probe changelog or raw delegate logs directly; they could give private experience new readers. Run mongodump before any migration and verify the round trip on a copy.

Still undecided: #84, which growth fields may change automatically.