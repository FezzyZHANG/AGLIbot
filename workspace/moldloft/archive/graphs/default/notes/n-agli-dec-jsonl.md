---
id: n-agli-dec-jsonl
graph_id: default
version: 1
title: Run v1 on the JSONL store; MongoDB later
kind: decision
state: accepted
in_graph: true
summary: '首版用 JSONL/JSON 文件存储，Store trait 已按 Mongo 集合设计，之后换实现即可。


  v1 uses file storage behind a Mongo-shaped Store trait; MongoDB becomes a second
  implementation.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: ticket
  target_id: 1
---
MongoDB is not installed here. The Store trait mirrors the planned collections (messages, model_calls, episodes, persons, lessons, audit, reviews, kv), so Mongo is a new implementation, not a redesign. Mutable documents are rewritten atomically.