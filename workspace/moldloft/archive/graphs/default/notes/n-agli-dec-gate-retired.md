---
id: n-agli-dec-gate-retired
graph_id: default
version: 1
title: The in-process evolution gate is retired; L4 is config/ and boundaries
kind: decision
state: accepted
in_graph: true
summary: '主人 2026-11-23 签字接受 0fe9648 删除 gate.rs：bot 不再自己提交、发布或重启；核心代码走 Moldloft →
  主人指定实现者 → Git → ops/deploy.ps1；L4 只剩 config/ 和 boundaries，恢复闸门本身是主人发起的 L4 变更。


  The owner accepted the deletion of src/evolution/gate.rs in 0fe9648. The bot no
  longer commits, publishes or restarts itself; core code goes through Moldloft, an
  owner-assigned writer, Git and ops/deploy.ps1. L4 is config/ and boundaries.'
created_at: '2026-11-23T17:28:44Z'
updated_at: '2026-11-23T17:28:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-hands
- relation: relates
  target_type: ticket
  target_id: 26
- relation: relates
  target_type: ticket
  target_id: 27
---
Retired with the gate: src/evolution/{gate,process,restart}.rs and src/agli/evolution.rs. /evolve and /evolve-status reply '暂未开放'.

Core code (L3): AGLI or anyone files a Moldloft ticket or proposal; the owner assigns a writer under root AGENTS.md; the writer builds and tests in their own environment; the owner reviews the diff; Git commit; ops/deploy.ps1. The writer allowlist is upheld by the owner's assignment and review, not by code. Bringing any in-process gate back is itself an L4 change the owner starts.

Docs synced 2026-11-23: AGENTS.md, DESIGN sections 2a/6/9, EVOLUTION.md, README.md, DEPLOY.md, CONFIG.md. The live store has no legacy Evolve review items. #27 (OS write protection) keeps its narrower scope and stays parked.