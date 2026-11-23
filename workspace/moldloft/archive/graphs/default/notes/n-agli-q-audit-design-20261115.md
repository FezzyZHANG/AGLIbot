---
id: n-agli-q-audit-design-20261115
graph_id: default
version: 1
title: Remaining design and documentation questions from the audit
kind: question
state: open
in_graph: true
summary: '待定设计包括历史群记忆、处理策略继承、证据等级、知识召回、预算和 Moldloft 提案采信，另附文档校正与工具检查。


  Open design questions, known deferrals and the limits of the current documentation
  and quality checks.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-q-growth-deidentification
- relation: relates
  target_type: note
  target_id: n-agli-src-review-20261115
- relation: relates
  target_type: ticket
  target_id: 17
- relation: relates
  target_type: ticket
  target_id: 21
- relation: relates
  target_type: ticket
  target_id: 27
- relation: relates
  target_type: ticket
  target_id: 29
- relation: relates
  target_type: ticket
  target_id: 44
- relation: relates
  target_type: ticket
  target_id: 47
- relation: relates
  target_type: ticket
  target_id: 53
- relation: relates
  target_type: ticket
  target_id: 57
- relation: relates
  target_type: ticket
  target_id: 69
- relation: relates
  target_type: ticket
  target_id: 84
- relation: relates
  target_type: ticket
  target_id: 98
---
Data policy. DESIGN §§2b/2d distinguish disclosure audience and processor. Several records derive LocalOnly from SelfOnly. Per-record policy and namespace/provider overrides remain incomplete. Widen review currently requests Any even if extraction required a narrower processor. Mixed journals and reloading old restricted derivatives after a provider change need explicit rules in #84. Decide whether ignored_groups only stops new intake or should affect past journal, reciprocity and member recall.

Evidence and knowledge. #44 authenticates that the job targeted a URL. The model-provided excerpt and its SHA-256 identify saved bytes. A source-authenticity claim needs a distinct original retrieval receipt. #17 generates knowledge files. User-facing chat retrieval of these cards remains a separate design question.

Operations and records. Ordinary budget routing, pinned non-exempt models, exempt calls, read-only sampling and the separate DSH budget have different controls. A global hard spending guarantee is still a design target. Update the design tree to distinguish persona seeds under mind/state from runtime authority under data_dir/state. Preserve the deferred cross-crash work in #29/#40/#41 and the owner's parked OS isolation in #27.

Multimodal. docs/research/sticker-store.md records the QQ-only sending decision and its superseded first-stage waiting period. Older research assumptions about tag fallback, schema, and localhost management differ from the current implementation and DEPLOY.md. Recognition quality, false collection, appropriate sharing, NapCat delivery, measured image accuracy and latency still require acceptance in the original multimodal groups.

Moldloft notebook adoption. MCP.md disallows an agent proposing an adopted premise. The reducer currently accepts an adoption proposal as pending, subject to human apply. Decide whether to restrict what may be proposed or define human approval of an explicit adoption as sufficient. Preserve human control in both interpretations.

Repository quality. The source audit found seven strict Clippy diagnostics and formatting differences in 66 files (1,679 diff blocks). EVIDENCE.txt records the exact files and rules. There was no repository CI workflow or pinned Rust toolchain at this baseline. The six documentation corrections in README, CONFIG, DESIGN, DSH, DEPLOY and EVOLUTION address actual configuration, group collection, media handling, routing, evidence and lifecycle. Core behavior and the open design questions have separate acceptance work.