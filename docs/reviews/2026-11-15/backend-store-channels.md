# Backend, storage, channels, and media review — 2026-11-15

This is a read-only source audit of the checkout. Findings below are supported by control-flow and state-transition evidence; no live bot, real chat, private data, or paid model was exercised. The root audit owns the complete test-run results. No core code or configuration was edited.

## Findings

### B01 — P1: Sending an offered sticker does not recheck its current scope or disabled status

**Primary location:** `src/agli.rs:1578-1593`. Related selection: `src/mind/expression.rs:27-38`; web edits: `src/stickers_web.rs:230-254`.

The offer captures a complete `MediaRecord` when retrieval runs. `usable` checks Active and Any/origin only at retrieval time. The offered pixels then pass through the chat model; this can take tens of seconds. If the owner disables the sticker or changes Any to Origin in the independent web process while the model is answering, `send_sticker` reads the old offer's file and sends it without loading the current record. The fresh read at lines 1596-1599 is after delivery and only updates usage counters. A disabled sticker can therefore still be sent, including across a group boundary the owner just closed.

**Required change:** Immediately before displaying/sending an offer, reload its current record and recheck status, scope, current chat, and file existence. For a truly concurrent revocation guarantee, coordinate the last check and dispatch or explicitly define that boundary.

**Acceptance:** Pause the fake model after retrieval; disable/change scope/delete the record; resume. No sticker is sent to the prohibited chat and ordinary reply text remains deliverable. Also test deletion before the usage-counter write. Static evidence; this interleaving was not run against live services.

### B02 — P2: Media metadata uses whole-record read/replace and can undo owner decisions

**Primary location:** `src/mind/media.rs:434-443`. Related writers: `src/stickers_web.rs:230-254`, `src/agli.rs:1596-1599`, `src/store/mongo.rs:399-401`.

When a previously known image arrives, the worker reads the whole record, changes sightings/last_seen, and saves the entire record. Web PATCH independently reads and saves the whole record. A valid interleaving is: worker reads Active/Any; web saves Disabled/Origin; worker saves its stale Active/Any plus new sighting. MongoDB's atomic document update does not prevent this lost update because it stores the entire serialized record under `$set: {record: ...}`. The same issue affects owner descriptions, deletion (stale upsert resurrects a record), counters, and concurrent recaption.

**Required change:** Provide field-specific mutations and atomic increments, or versioned compare-and-set with retry/merge. Deletion must not be recreated by a stale counter/sighting write.

**Acceptance:** Use two store instances and a barrier between read and save; interleave disable/scope change/edit/delete with sighting and sent-count updates. Owner decisions persist and counts accumulate. Static interleaving evidence; not a live reproduction.

### B03 — P2: The documented separate web process is unsafe with the supported JSONL backend

**Primary location:** `src/store/jsonl.rs:122-139`, `src/store/jsonl.rs:335-339`. Architecture: `src/stickers_web.rs:47-65`; `docs/DEPLOY.md:51`.

JSONL mutable collections are read, modified, and rewritten under a `std::sync::Mutex` belonging to one `JsonlStore` instance. Bot and stickers-web each open their own instance in different processes. Both write the same fixed `.media.json.tmp` (and `.sticker_vectors.json.tmp`). Concurrent writes can lose records from different hashes, rename bytes written by the other process, or fail because the other process already renamed the shared temporary path. A successfully acknowledged owner's edit can subsequently disappear.

This differs from B02: even writers modifying unrelated hashes collide through the whole-file write and shared temporary filename; MongoDB avoids this specific file-level fault.

**Required change:** Add a shared OS/file lock spanning load-modify-replace for every writer and unique temporary filenames, or make JSONL explicitly single-writer and route web mutations through the bot. If JSONL web mode is unsupported, reject it clearly instead of silently permitting unsafe operation.

**Acceptance:** Two independent store handles/processes repeatedly update different media records and vectors; all acknowledged updates survive reopening, no malformed JSON or rename failure, and no temporary-path collision occurs. Static proof from lock ownership and write algorithm; no user files touched.

### B04 — P2: import-jsonl silently omits authoritative lexicon and media records

**Primary location:** `src/store/import.rs:17-29`, `src/store/import.rs:37-49`; source snapshot: `src/store/jsonl.rs:44-61`; target snapshot: `src/store/mongo.rs:133-149`.

The importer snapshots and verifies only the original ten collections. The current Store contract also has `lexicon`, `media`, and `sticker_vectors`. Lexicon and media records contain owner-authored aliases, meanings, edits, status, and scope decisions, so they are not rebuildable cache. An import can print successful verification while losing them. Files may remain in data/media, but without media metadata the bot cannot use the collection and can re-collect disabled images. Vectors may be rebuilt, but authoritative text/status/scope cannot.

The synthetic store contract already creates lexicon entries, while Snapshot excludes them, so the existing importer tests can pass despite dropping the new collection.

**Required change:** Expand snapshot, preflight comparison, write, and verification to all authoritative collections. Explicitly document/skip only derivable caches. Preserve a migration route for pre-media installations.

**Acceptance:** Seed alias/slang entries plus Active/Disabled/Origin media with owner-edited text, import to a disposable Mongo database, and compare all records and disabled behavior after reopen. Dry-run must detect conflicting preexisting authoritative data. Static schema-coverage evidence; no Mongo instance opened for this audit.

### B05 — P2: Files whose description fails or whose budget is exhausted escape the seven-day cleanup

**Primary location:** `src/mind/media.rs:428-458`; cleanup: `src/mind/media.rs:528-535`.

The worker stores the downloaded file at line 432 before normalization/description. On exhausted budget or any description/JSON/format/model failure, it exits without creating a MediaRecord. The cleanup only traverses `store.all_media()`, so these files are never considered and survive indefinitely unless the same content later succeeds or is manually removed. The existing `a_failed_description_keeps_the_file_and_leaves_no_description` test (lines 732 onward) explicitly proves the file-without-record state but does not test aging it through pruning.

This contradicts the seven-day retention promise for files not kept as stickers in `docs/research/sticker-store.md:96-104` and increases private image retention/disk usage during provider failures or daily budget exhaustion.

**Required change:** Persist a retryable/non-sticker record before fallible work, or prune old orphan files using a durable last-seen ledger and safe file age checks. Preserve retry-on-next-sighting behavior.

**Acceptance:** Failed/invalid/budget-denied image files age beyond seven days and are removed; active stickers remain; a recent reappearance extends retention; no broad deletion of unknown files.

### B06 — P2: A timed-out merged-forward send is retried as ordinary QQ messages

**Primary location:** `src/channels/qq.rs:237-259`, especially 249; timeout classification: `src/channels/qq.rs:222-230`.

For text above two chunks, send_reply first sends a merged forward. Every error — an explicit rejection, a missing acknowledgement after delivery, or a dropped connection — triggers fallback to ordinary chunks. When NapCat/QQ accepted the forward but its response was lost/delayed, this sends the same response twice. It is an automatic repeat after an unknown external outcome, conflicting with the design's audit/recovery boundary and the sticker-store sending contract.

**Required change:** Distinguish explicit rejection from unknown delivery. Fall back only after a rejection that proves no send occurred; propagate Unknown on timeout/disconnect and do not repeat it.

**Acceptance:** Fake OneBot accepts the forward but withholds its reply until timeout: no normal chunks are sent. An explicit unsupported/rejected action may fall back. Existing long_forward_actions_nodes_cache_and_fallback tests cover explicit errors, not this uncertain outcome.

### B07 — P2: Expired QQ action requests can execute after reconnection; timed-out pending entries never expire

**Primary location:** `src/channels/qq.rs:222-230`, `src/channels/qq.rs:416-442`, `src/channels/qq.rs:467-475`, `src/channels/qq.rs:487-493`.

The call timeout starts after enqueuing. During a long disconnect backoff (up to 60 seconds), a request can enter the queue and time out after 20 seconds. The run loop drains the queue before the backoff, then reconnects before draining again. On successful reconnect, session blindly sends the expired queued request, without checking `req.reply.is_closed()` or a deadline. A caller that already reported failure/unknown can therefore cause a delayed external send. Waiting to enqueue into a full queue has no deadline at all.

Additionally, pending entries are removed only by an echoed response or full session termination. A connected endpoint that never answers an action leaves each timed-out oneshot sender in the HashMap permanently. Ongoing image/group checks accumulate these entries.

**Required change:** Carry an absolute per-request deadline; bound enqueue plus send plus acknowledgement under it; discard canceled/expired requests before dispatch; expire pending entries without retrying any possibly sent action.

**Acceptance:** Paused-time fake transport: enqueue during 60-second backoff, let the caller time out, reconnect, and assert no action dispatch. Repeat unanswered actions on an open connection and assert bounded pending state. Static lifecycle evidence; no real OneBot manipulation.

## Documentation alignment

- DESIGN.md:47 still says stickers are not integrated, although media collection, retrieval, and QQ sending are present.
- docs/research/sticker-store.md:3 already explicitly records owner authorization for QQ-only sending on 2026-11-07, supersedes the original Stage 1/Stage 2 plan, and states no per-chat limit or tag fallback. These are accepted changes, not missing authorization or defects. Historical proposed collection names (stickers/sticker_embeddings) differ from current media/sticker_vectors; the planned per-occurrence media provenance is represented by chat transcripts rather than MediaRecord. A clearer current-state section would reduce ambiguity.
- sticker-store.md says localhost-only and authentication required before phone access at line 133; the later DEPLOY.md and actual listener allow any private IPv4 LAN peer without login. The unauthenticated LAN design is explicitly documented in the current deployment guide, so this is conflicting documentation, not an invented auth requirement.
- sticker-store.md's original body calls for a similarity threshold and tag fallback; expression.rs deliberately has no similarity floor, and the opening update explicitly accepts no tag fallback. Treat retrieval quality as an unverified evaluation target, not a proven code defect.
- Telegram's source comment claims the limit is UTF-16 units, while official TDLib checks utf8_length; chars-based splitting was not classified as a defect. Primary cross-check: https://raw.githubusercontent.com/tdlib/td/master/td/telegram/MessageContent.cpp (bot text length check around line 4990).

## Reviewed coverage and positive checks

Read production paths and focused tests in src/store/{mod,cache,jsonl,mongo,import,tests}.rs; src/channels/{mod,telegram,qq}.rs; src/stickers_web.rs and stickers_web.html; src/mind/{media,vision,embedding,expression}.rs; embedding/tests.rs; and relevant AGli intake, retrieval, sending paths. QQ's test section was inspected by named scenarios and selected bodies, rather than executed independently of the root's full suite.

Verified by source: memory archive rejects cross-namespace/scope summaries; Mongo keeps lossless JSON timestamps/u64 values and filters BSON millisecond boundaries again in Rust; invalid configured Mongo does not silently select JSONL; source caches invalidate with mutations and are explicitly in-process; web list/files/edit/bulk hide Seen records; web guard checks peer, Host, and a preflight-requiring write header; model-written text enters the UI through textContent rather than HTML; hashes and supported file extensions constrain media access; media downloads cap both declared and streamed bytes; vision caps compressed size, pixels, frame budget, pending references and image parts; episode embedding checks processing policy and vector validity.

Limits: no production DB/log sweep, no credentials, no paid/network model tests, no real Telegram/NapCat delivery, and no Mongo integration run. Root owns Cargo/build execution evidence. Broad backend concurrency findings are static interleavings unless explicitly reported otherwise.
