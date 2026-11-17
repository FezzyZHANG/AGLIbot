---
id: "n-agli-dec-audit-p1-20261115"
title: "Owner disposition of the 2026-11-15 P1 audit findings"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "ticket:84"}, {"type": "relates", "target": "ticket:85"}, {"type": "relates", "target": "ticket:86"}, {"type": "relates", "target": "ticket:87"}, {"type": "relates", "target": "ticket:26"}, {"type": "relates", "target": "note:n-agli-q-growth-deidentification"}, {"type": "relates", "target": "note:n-agli-dec-two-layer"}, {"type": "relates", "target": "note:n-agli-dec-runtime-root"}]
abstract_zh: "主人反馈：A01/A02 先核对脱敏与成长设计，不默认接受原修法；A03/A05 认可并登记修复；A04 将进化退回返工；A06 要求独立超级开关，后端可能重构。只记录方向与待办，没有实现或上线。"
abstract_en: "The owner requested design reconciliation for A01/A02, accepted A03/A05, returned evolution to rework for A04, and required an independent master kill switch with possible backend refactoring for A06. This records directions and open work, not implementation, writer assignment or deployment."
---
## Owner disposition — 2026-11-15

This note records the owner's response to the repository audit, not an implementation closeout. Source: docs/reviews/2026-11-15/REVIEW.md, baseline 0117d6f385b5f63004e72403d51fc8932aeeaf76.

- A01/A02: carefully reconcile the de-identification design. Keep the information-flow evidence, but do not treat the audit's suggested mechanism or a blanket ban on abstract growth as accepted. Open question n-agli-q-growth-deidentification and design-review ticket #84 retain this uncertainty.
- A03: accepted finding; track atomic review creation and stable approval identity in #85.
- A04: return the evolution feature to rework. The owner judged completion clearly inadequate. Reopen #26 at High priority for feature-level lineage, lifecycle, release/PID/preflight integration and renewed end-to-end acceptance. Do not reduce this direction to adding one missing ancestor check.
- A05: accepted finding; track both dispatch-time revocation and stale metadata overwrite in #86.
- A06: an independent master kill switch is required. The owner judged the backend control design seriously flawed and considers refactoring potentially necessary. #87 must define the switch scope and independent cancellation/deadline architecture before broad implementation. No particular concurrency topology is selected here.

## Boundaries

All new work remains Open / High, not In progress or Done. No core implementation, writer appointment, L4 change, live configuration update, deployment or process restart was performed. The core-writer and owner-only L4 rules continue to apply. #27 filesystem isolation remains separately parked; a backend control redesign does not implicitly revive it.

The earlier completion evidence in #5/#20/#36 is preserved with cross-references; it does not settle the newly identified design/control boundaries. #26's historical implementation evidence is retained but its Done acceptance is withdrawn by the owner's explicit rework instruction.

Other audit findings retain their recorded state; this note does not claim the owner has accepted all 24 findings or commissioned every P2 fix.
