---
id: n-ev-prompted-delegation-unreliable
graph_id: default
version: 1
title: Prompt-driven delegation is unreliable; structural separation is not
kind: evidence
state: recorded
in_graph: true
summary: '文章实验：单会话靠提示词决定是否委派很不稳定（中性提示下 5 次都没委派）；Router/Worker 结构分工的完成时间更短、更稳，代价是更多
  token。合成负载，仅作初步参考。


  Essay experiment: delegation left to prompting is unstable (0 of 5 under a neutral
  prompt); structural Router/Worker separation finished faster and more consistently
  at higher token cost. Synthetic, preliminary.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-src-deepseek-superagent
- relation: supports
  target_type: note
  target_id: n-agli-dec-two-layer
- relation: supports
  target_type: ticket
  target_id: 37
---
Essay section 7.1 (synthetic load, about 300 messages, about 20 work requests, 5 runs each): a single session with a subagent tool and a neutral prompt never delegated in 5 runs (p95 29.5 min); with a delegate-first prompt it delegated in 4 of 5 and still had a long tail. Router + Worker: p50 12.4 / p95 15.7 min, at higher cache-read tokens (23.23M per run). Removing direct delivery kept completion time but raised Router decisions about 30% and cache reads 37%. Limits: synthetic load, coding replaced by a sleep tool, 5 runs; the authors call it preliminary.