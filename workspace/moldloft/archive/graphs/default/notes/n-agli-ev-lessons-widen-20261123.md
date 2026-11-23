---
id: n-agli-ev-lessons-widen-20261123
graph_id: default
version: 1
title: 'Live lessons review: single-pass auto-widening is not safe as is'
kind: evidence
state: recorded
in_graph: true
summary: '线上 250 条自动公开的 lessons 中 48 条不应原样公开；审查理由多为套话；公开副本在 590a346 从未进入聊天。


  Of 250 lessons the reviewer auto-widened, 48 should not stay public as written;
  reasons are mostly boilerplate; at 590a346 the public copies never reached a chat.'
created_at: '2026-11-23T17:28:44Z'
updated_at: '2026-11-23T17:28:44Z'
adopted_body_hash: null
links:
- relation: challenges
  target_type: note
  target_id: n-agli-dec-widen
- relation: relates
  target_type: ticket
  target_id: 110
- relation: relates
  target_type: ticket
  target_id: 112
---
Read-only export of the live store (590a346), 2026-11-23: 565 lessons (310 self, 255 public copies); 306 Widen reviews: 250 auto-passed, 53 forwarded (5 approved, 1 rejected, 47 open), 3 revise. gemini-4-argon made 308 of 313 verify calls. Astra (gpt-6.2-astra, xhigh) reviewed all 297 auto-passed or open items; Opus checked the four items that needed source context locally, without sending source text to an outside model.

Results: auto-passed 202 keep, 7 retract, 41 rewrite; open 28 approve, 2 reject, 17 approve after rewrite. Fifteen open items are reviewer call failures (10 were HTTP 524), not judgements.

Failure pattern: the reviewer looks for explicit identifiers (232 of 250 reasons mention time, 173 list time, relationship and event together) but does not check whether acquaintances could reassemble the event, never reasons about what the original teller would mind, and passes lessons that label people, turn someone's private life into a social technique, treat bypassing safety limits as a lesson, or treat privately discussed research as public knowledge. Verdict::parse folds unclear output into 'forward to owner', and the raw output is not stored.

Exposure: chat recall at 590a346 passes include_lessons: false and privacy::recall reads lessons only for a solo audience, so the public copies gained permission but were never said in a chat.

Summary: docs/reviews/2026-11-23/lessons-widen-review.md. Per-item list (paraphrases private chats, gitignored): data/reviews/2026-11-23-lessons-widen.md.