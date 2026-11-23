---
id: n-agli-dec-widen
graph_id: default
version: 1
title: Scope widening applies on a passing review
kind: decision
state: accepted
in_graph: true
summary: '扩大可见范围的经验经审查“通过”即生效（另存公开副本并引用原条目），其余情况保持私有或转交主人。


  Widening takes effect on a passing review as a separate public copy; otherwise it
  stays private or goes to the owner.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-q-growth-deidentification
---
Following DESIGN 2d/5d, a lesson the model marks shareable is reviewed by judge (verify scenario). 通过 creates a separate public copy that references the self-scoped original; other verdicts keep it self-only or forward it to the owner. If no reviewer model is available, it goes to the owner.