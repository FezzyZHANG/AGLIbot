---
id: n-agli-dec-no-cloud-split
graph_id: default
version: 1
title: No cloud/daemon split for now
kind: decision
state: accepted
in_graph: true
summary: '暂不采用云端无状态 + 端侧 daemon：AGLI 是桌面 agent，拆分只会增加攻击面。迁到常驻服务器时再议。


  Not adopting the cloud loop + device daemon split while AGLI is a single-machine
  desktop agent.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-src-deepseek-superagent
---
Rejected for now: essay section 5 (stateless cloud agent loop, state in a database, a lightweight daemon on user machines, BYOM). AGLI is a desktop agent whose persona, secrets and execution environment already live on one machine; splitting them adds the attack surface the essay itself flags in section 8.4. Revisit if AGLI moves to an always-on server.