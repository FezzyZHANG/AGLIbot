---
id: n-agli-src-review-20261115
graph_id: default
version: 1
title: AGLIbot repository audit of 15 November 2026
kind: note
state: active
in_graph: true
summary: '本轮复验的源码、报告、夹具与覆盖范围，供逐项追溯。


  Source baseline, reports, fixtures and coverage for the November 15 re-acceptance.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-audit-p1-20261115
---
Repository commit 0117d6f385b5f63004e72403d51fc8932aeeaf76. Use docs/reviews/2026-11-15/REVIEW.md for stable A01–A22/M01–M02 identifiers and locations, and EVIDENCE.txt for the retained results. The report and documentation corrections are present in the working tree.

The component reports are mind-social-privacy.md, backend-store-channels.md, hands-evolution-ops.md and moldloft-backend.md in that directory. The Rust audit_probes.rs fixture, ops-mock-check.ps1 and Moldloft fixture accompany the evidence. Four Rust checks assert current failure modes. Passing one of these original checks demonstrates the observation.

The audit covered the main project's Rust modules and selected Moldloft backend paths. It did not establish full coverage of the embedded tool, current production health, Mongo migration in its present schema, live model availability, or end-to-end browser behavior. Refer to dated ticket receipts for historical operational evidence.