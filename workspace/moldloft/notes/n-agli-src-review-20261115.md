---
id: "n-agli-src-review-20261115"
title: "AGLIbot repository audit of 15 November 2026"
kind: "source"
state: "recorded"
relations: [{"type": "relates", "target": "note:n-agli-dec-audit-p1-20261115"}]
abstract_zh: "本轮复验的源码、报告、夹具与覆盖范围，供逐项追溯。"
abstract_en: "Source baseline, reports, fixtures and coverage for the November 15 re-acceptance."
---
Repository commit 0117d6f385b5f63004e72403d51fc8932aeeaf76. Use docs/reviews/2026-11-15/REVIEW.md for stable A01–A22/M01–M02 identifiers and locations, and EVIDENCE.txt for the retained results. The report and documentation corrections are present in the working tree.

The component reports are mind-social-privacy.md, backend-store-channels.md, hands-evolution-ops.md and moldloft-backend.md in that directory. The Rust audit_probes.rs fixture, ops-mock-check.ps1 and Moldloft fixture accompany the evidence. Four Rust checks assert current failure modes. Passing one of these original checks demonstrates the observation.

The audit covered the main project's Rust modules and selected Moldloft backend paths. It did not establish full coverage of the embedded tool, current production health, Mongo migration in its present schema, live model availability, or end-to-end browser behavior. Refer to dated ticket receipts for historical operational evidence.