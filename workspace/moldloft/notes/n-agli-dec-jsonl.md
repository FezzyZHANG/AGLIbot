---
id: "n-agli-dec-jsonl"
title: "Run v1 on the JSONL store; MongoDB later"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "ticket:1"}]
abstract_zh: "首版用 JSONL/JSON 文件存储，Store trait 已按 Mongo 集合设计，之后换实现即可。"
abstract_en: "v1 uses file storage behind a Mongo-shaped Store trait; MongoDB becomes a second implementation."
---
MongoDB is not installed here. The Store trait mirrors the planned collections (messages, model_calls, episodes, persons, lessons, audit, reviews, kv), so Mongo is a new implementation, not a redesign. Mutable documents are rewritten atomically.