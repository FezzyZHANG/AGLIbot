---
id: n-agli-dec-privacy-files
graph_id: default
version: 1
title: Personal runtime content stays out of git
kind: decision
state: accepted
in_graph: true
summary: '运行时状态、任务目录、看板和日记不进 git，避免私聊内容进入仓库；mind/state 只保留种子。


  Runtime state, tasks, board and journal are gitignored so private content never
  reaches the repository.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: ticket
  target_id: 8
---
Runtime state moved from mind/state to data/state (mind/state keeps seeds). work/tasks, work/board.yaml and mind/life/journal are gitignored. This trades the 'git-versioned consciousness' idea for not leaking private conversations into the repository.