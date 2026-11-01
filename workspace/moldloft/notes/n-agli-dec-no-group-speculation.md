---
id: "n-agli-dec-no-group-speculation"
title: "Others may call published capabilities only; no speculative work from group messages"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "note:n-src-deepseek-superagent"}, {"type": "relates", "target": "note:n-agli-dec-intent"}, {"type": "relates", "target": "note:n-agli-dec-groups"}]
abstract_zh: "群友和其他 agent 可以调用已经程序化发布的能力；更高级的调用（委派 hands、DSH、自由任务）和任何修改都不对外开放，只能进 board 等主人确认。不因群消息投机开工，主动接活先走 observe 模式。"
abstract_en: "Non-owners may invoke programmatically published capabilities; advanced invocations and any modification stay owner-only. No speculative work from group messages; self-initiated task-taking starts in observe mode."
---
Refines DESIGN section 6 ("group members and agents cannot trigger tools").

Published capability: an entry the owner registered in code or config (L4) with a fixed command name, a typed argument schema, a hard cost cap and no side effects beyond answering the requester. It runs on the deterministic command path (n-agli-dec-intent), never on reply markers or the model's reading of a message. Examples: looking up a finished research card, a fixed-format summary of public group history, a status query.

Not open to non-owners, whatever they say:
- advanced invocation: delegating to hands (codex, cc, DSH), free-form research or code tasks, web fetches with requester-chosen targets, anything without a fixed schema and cost cap;
- modification: config, boundaries, memory/persona edits, board state other than appending a request, code or evolution proposals.
Such requests go to the board for the owner, as before.

Each published capability carries its own ACL (which groups/people) and rate limit, inherits the read-only/allowlist split (n-agli-dec-groups: no replies in read-only groups, so no capability calls there either), and is subject to privacy.rs like any reply.

Speculative work: the essay's "silently reproduce and fix" policy starts work from public group messages; that stays forbidden. If AGLI ever takes work on his own initiative, start in observe mode: log what he would have started, review the false-positive rate via the daily digest, then enable. The prototype's proactive acceptance also defaults to observe (src/policy.ts:84).