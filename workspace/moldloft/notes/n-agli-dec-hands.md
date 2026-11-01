---
id: "n-agli-dec-hands"
title: "Delegation: Codex for routine work; Claude Code is the owner's; core code goes through Moldloft"
kind: "decision"
state: "proposed"
relations: [{"type": "relates", "target": "ticket:22"}, {"type": "relates", "target": "ticket:26"}, {"type": "relates", "target": "note:n-agli-dec-two-layer"}]
abstract_zh: "Claude Code 只归主人用，AGLI 不调用。常规研究交给 Codex。涉及核心代码的工作走 Moldloft 辅助层：AGLI 只提交工单/提案，主人审阅后再动手。只有 Astra 和 Opus 能直接写核心代码；deepseek-v4.1-flash-1017 只能做由 Astra 或 Opus 指导的良定义任务。"
abstract_en: "Claude Code is reserved for the owner. Codex handles routine research. Core-code work goes through the Moldloft layer as reviewed tickets/proposals. Only Astra and Opus write core code directly; deepseek-v4.1-flash-1017 only for well-defined tasks directed by Astra or Opus."
---
Layers, from AGLI outward:

1. AGLI (persona) talks and dispatches; never executes (n-agli-dec-two-layer).
2. Codex is his default hand for routine research and workspace tasks: subprocess with the prompt on stdin, --sandbox workspace-write in the task dir, --search for research. Only owner-approved tasks reach delegation; requests from others become review items.
3. Moldloft is the auxiliary layer for core code (src/, L3 and up): AGLI may file tickets and submit ticket/note proposals through Moldloft MCP, with evidence and a scoped spec. He gets no approval tool; the owner reviews and decides who implements.
4. Claude Code is the owner's tool. AGLI does not invoke the claude CLI at all; that quota and authority stay with the owner.

Core-code writer allowlist (enforced in delegation code, not by prompt):
- Astra (gpt-6-astra via Codex) and Opus (via the owner's Claude Code) may write core code directly, after owner approval of the Moldloft ticket.
- deepseek-v4.1-flash-1017 only for well-defined tasks, meaning tasks directed by an advanced model: Astra or Opus writes the plan, names files, interfaces and deciding tests, and reviews the diff before the owner sees it. On ambiguity the directed model stops and hands back.
- Any other model may read and propose, never write core code.
The working rule lives in the root AGENTS.md so every coding agent sees it.

Consequences: ticket 22 (re-enable claude for research) becomes removing the claude path from src/hands/delegate.rs. AGLI's hands only write in work/tasks/<id>/, so the allowlist is enforced where core code is actually changed: the evolution gate (ticket 26). The L3 row of DESIGN section 6 routes proposals through Moldloft review.