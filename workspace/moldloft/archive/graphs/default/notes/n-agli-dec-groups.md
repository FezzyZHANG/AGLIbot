---
id: n-agli-dec-groups
graph_id: default
version: 1
title: Joined groups are read-only by default; only allowlisted groups get replies
kind: decision
state: accepted
in_graph: true
summary: '默认只读：加入的群都静默学习（记录、夜间反思），但不接话；只有白名单里的群才走发言流程。


  Read-only by default: every joined group is learned from silently; only allowlisted
  groups go through the reply path.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-no-group-speculation
- relation: relates
  target_type: ticket
  target_id: 6
---
Supersedes the earlier rule that non-allowlisted groups are neither read nor stored.

Every group the account has joined is read-only by default: messages are stored under that group's namespace and scope and feed nightly reflect, memory and person cards. Read-only groups never reach floor, triage or speak, so they cost no per-message model calls and he never posts there, not even when @-mentioned or replied to.

config/social.yaml `groups` becomes the reply allowlist; only those groups go through floor and may get replies. Moving a group between the two is an owner config edit (L4).

Guards that stay in force for read-only groups:
- Group content remains untrusted data and still triggers nothing (see n-agli-dec-no-group-speculation).
- privacy.rs scope rules apply as for any group: what is learned in one group is not repeated in another chat.
- Reflect over read-only groups needs its own token budget cap, so a large or noisy group cannot crowd out the owner's chats; drop to sampling when it is exceeded.
- A group can be marked ignored (not stored at all) for groups the owner does not want recorded.