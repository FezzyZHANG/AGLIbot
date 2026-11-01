---
id: "n-agli-dec-two-layer"
title: "Persona is the router, hands are the workers; enforced in code"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "note:n-src-deepseek-superagent"}, {"type": "relates", "target": "ticket:32"}]
abstract_zh: "人格主循环相当于 Router，只说话和分派，不挂执行工具；hands 相当于 Worker，不直接对人。由代码保证，不靠提示词。"
abstract_en: "The persona loop is the router (talks and dispatches, no execution tools) and hands are the workers (never address people). Enforced in code rather than by prompt."
---
AGLI keeps the Super-agent split: the persona loop only talks and dispatches, never gets execution tools, and hands never talk to people; results return through the persona. Enforced in code (converse() has no tools; hands start only through board -> job), not by prompt: the prototype gives its Router bash with danger-full-access and relies on prompt rules.