# Moldloft backend audit supplement — 2026-11-15

This supplement is a focused review of the embedded independent Moldloft source and its AGLI installation boundary. It does not claim a complete Moldloft frontend, packaging, hooks, or distribution audit. No live graph, notebook, proposal, configuration, hooks, credentials, or running service was changed.

## M01 — P2: Applying a proposal overwrites the retained review diff using the already-applied graph

**Location:** `Moldloft/src/tickets/proposals.py:313`, `Moldloft/src/tickets/proposals.py:414-423`; renderer: `Moldloft/src/tickets/review.py:113-160`.

Submission computes the original diff before changing data and writes it to the review Markdown. After apply commits, `render_file(outcome)` recomputes changes by previewing the original operations against the current, already-updated graph. For an ordinary Before -> After update, that preview has no changes; it rewrites the review file with an empty Changes section. The original prior text is not stored in the proposal record, so it cannot be reconstructed from the DB. For deleted targets/create conflicts, the file instead says the differences can no longer be rendered. With reviewer amendments, the recomputed preview may show a change from the approved text back to the agent's unamended proposal, which is the opposite of the performed action.

This undermines the retained human review form promised by MCP.md:208-214 and the development contract. The application does not commit files automatically, so saying a previous Git revision retains the pending diff is not guaranteed.

**Reproduction run:** Real temporary SQLite Store/Proposals and production renderer, replacing only the filesystem rendering sink with an in-memory string collector. The pending renderer contained `- title: "Before"` and `+ title: "After"`; after successful apply neither remained, while the stored ticket was After. Source is unchanged. See `target/audit-20261115/moldloft-probes.py` and `moldloft-probe-results.txt`.

**Fix:** Capture original proposed and applied before/after review sections durably with the proposal/outcome, or preserve the existing original Changes block and append the outcome. Replays must render the same durable content, not use whichever graph exists later.

**Acceptance:** Update, delete, create, note-only, mixed, and amended proposals retain their original proposed diff and exact approved result after apply/reject/expiry/retry/restart, including when no intermediate Git commit exists.

## M02 — P2 security boundary gap: REST accepts arbitrary Host when Origin matches it

**Location:** `Moldloft/src/tickets/app.py:165-174`. Contrast `Moldloft/src/tickets/mcp_surface.py:146-154` and `project_runtime.py:103-110`, which configure the MCP transport's explicit allowed Hosts.

The parent HTTP middleware rejects a nonmatching Origin but trusts the attacker-supplied Host in `request.base_url`. There is no parent Host allowlist. A request with Host `attacker.example:8787` and Origin `http://attacker.example:8787` is accepted. MCP would separately reject an unapproved host, but ordinary `/api/tickets`, `/api/proposals/.../apply`, note writes, and HTML human forms do not traverse that MCP guard.

This leaves a DNS-rebinding route to the unauthenticated human API: if an attacker-controlled same-origin domain resolves to the local/LAN listener, Origin remains the attacker's domain and the comparison passes. The known intentional LAN/no-auth deployment is not itself classified as a defect; the finding is that the promised cross-site protection is incomplete and inconsistent between MCP and REST.

**Boundary-level reproduction run:** Directly drove the production `local_boundary` coroutine with a synthetic Starlette Request and an immediate downstream response (no network/server/database): arbitrary Host plus matching Origin returned downstream status 204; a different Origin returned 403. The guard has no separate branch enforcing host validity for REST. Evidence is in the same probe files.

**Limit:** No browser DNS-rebinding attack or network API mutation was executed. Browser local-network access restrictions may provide an independent mitigation; this audit does not assume that every browser permits the attack. An attempted ASGI TestClient verification could not start its event loop because the restricted Windows environment hung in `socket._fallback_socketpair -> accept`; both attempts were canceled. This is an environment limitation, not a Moldloft failure.

**Fix:** Validate Host for the entire parent app against explicit local and manifest-derived hostnames/addresses; then validate Origin against the validated authority. Cover REST, forms, and MCP consistently without removing the deliberate LAN setting.

**Acceptance:** Synthetic ASGI/browser fixtures reject unapproved Host even with a matching Origin; allow configured localhost/LAN hosts; preserve cross-origin write rejection and approved MCP endpoints.

## D01 — Documentation/implementation conflict: agent proposals may request adopted premise state

MCP.md:189 says an agent cannot propose adopting a premise. However `note_ops.py:84-89` accepts assumption/state=adopted and stamps adopted_digest, and `tests/test_note_ops.py:70-79` expressly expects this. Proposals.submit has no prohibition. An isolated real submit accepted a note_create with state adopted as pending, while no note was written.

This is not an approval bypass: the operation remains pending and still requires the human apply route. The owner must decide whether the intended rule is “agent cannot propose adoption at all” or “agent may propose it, explicit owner apply constitutes adoption.” Align MCP.md and tests to that decision; do not tighten behavior solely on this audit's inference.

## Positive checks and integration alignment

Read relevant sections of README, AGENTS, shared principles, DESIGN, MCP, and source in writes.py, db.py, drydocks.py, proposals.py, basis.py, notes.py, note_ops.py, review.py, app.py, mcp_surface.py, config.py, project_runtime.py, gitinfo.py, and copilot/LLM boundary paths. Inspected focused assertions in test_review, test_proposals, test_proposal_apply, test_mcp_surface, test_note_ops, and test_writes.

- Cross-process writer exclusion uses a separate SQLite BEGIN IMMEDIATE guard, unlike AGLI's instance-only JSONL mutex.
- Mixed proposal transactions journal prior and intended bytes plus assigned IDs before replacement; uncertain COMMIT checks the durable marker rather than blindly rolling back committed data.
- Recovery verifies canonical original notebook paths and refuses unrelated external bytes; unresolved attempts block ordinary writes.
- Proposal apply revalidates the captured immutable basis under the writer guard, preserves idempotent outcomes, and does not expose an MCP approval/apply tool.
- Drydock IDs select independent data stores and bases; missing additional storage is unavailable rather than initialized as a blank graph.
- Notebook IDs and regular-file paths are bounded, symlinks are rejected, files are replaced atomically, and external edits are checked before replacement.
- MCP read context/basis maps are bounded and visibly truncated; metadata navigation stays out of automatic premise expansion.
- Project mode disables paid model features and avoids implicit parent env/config adoption. The AGLI manifest explicitly selects a LAN bind; this is intentional current configuration, not a discovered unauthorized source change.
- No Rust code was found directly opening Moldloft SQLite. The source integration is the documented project launcher and independent workspace; live edits must continue through the application API.

Not covered comprehensively: frontend editing/layout, agent-hook install/runtime, wheel/sdist packaging, all maintenance/migration variants, real LAN firewall/browser behavior, all failure-injection tests, or full Moldloft unittest suite. No claims of full Moldloft test passage are made.
