# Gate #58: a self-collecting sticker store

2026-11-06. Stage 1 below is built (`src/mind/media.rs`, `src/stickers_web.rs`), which supersedes this note where they differ; stage 2 is not authorized. Extends [memes.md](memes.md), whose repository baseline is out of date: inbound images and stickers reach the chat model since #55 (see [vision.md](vision.md)). Sending a sticker is still not implemented; replies are text and @ only.

Probes were single requests from this machine against the live runtime's credentials and database. Prices come from `config/models.yaml`, which holds routing estimates, not quotations. External facts use numbered sources. No retrieval or filtering quality has been measured.

## Owner decisions, 2026-11-06

- **Collection is automatic.** The model collects what it sees and filters on its own, with no approval step. This replaces the owner-approved catalogue in the first draft of this note.
- **Search uses vectors of descriptions,** embedded with BGE-M3.
- **Rights are not a gate.** The rights rule proposed in [memes.md](memes.md) is dropped, and no rights field is kept.
- **Management is a web UI,** not chat commands.
- **Cached images are read first,** before any download.
- **Stickers may cross groups by default.** The model decides at collection whether one should stay in its origin group.
- **The pixel window is 15 minutes.**
- **In private chats, only what the owner sends is collected.**
- **Every image in collected groups may go to the Gemini relay.** The owner accepts this.

## Recommendation

Download and describe every image when it arrives, in a background worker separate from the chat loop. Store files by content hash, keep what the model judges reusable as stickers, and search those by description vector.

This is a bigger change than a sticker table, because it replaces the rule that images are only fetched when he is addressed. It should ship in two stages: collect silently first, send only once a week of real collection has been reviewed.

## What this replaces in image handling

| Today (#55) | With collection |
|---|---|
| Download only on an addressed turn | Download on arrival |
| Keep references only; no image on disk | Files in `<data_dir>/media/<sha256>.<ext>` |
| Links can expire; `get_image` refreshes them | Fresh links at arrival; refresh only for messages the bot never saw |
| Waiting images are lost on restart | Files and descriptions survive |
| Chiming in sees `[图片]` only | Chiming in sees the description |
| Later sessions see `[图片]` | Later sessions see the description |
| Quoted images are downloaded again each time | A quote resolves to the stored hash |
| Waiting list: 30 minutes, 4 per chat, pixels sent for all | Pixels only where the turn is about them (below) |

**Pixels on addressed turns stay,** as the owner chose: the main model reads the image itself, and the Flash description is only an index. What changes is which images go along:

- The image in the addressed message, and any it quotes, always go as pixels.
- Images from the last 15 minutes go as pixels, covering "小庄你看这个".
- Older ones go as description text, with their ids.

This removes the cost flagged when the wait was raised to 30 minutes: up to 4 unrelated images replaying at about 1,091 tokens each per later turn.

**The cache is read first.** A look or a quote finds the file through the message's `media` record, and downloads only on a miss, for example when the worker hasn't reached that image yet or the message predates the cache. The bytes it downloads go into the cache too.

The description cannot be written into the message at arrival, because it isn't ready yet. Each sighting goes into a `media` record keyed by message, and transcripts join it when the turn is built. If it isn't ready, the line keeps `[图片]`.

## Volume and cost

Counted from the live message store, Nov 3–5, full days only:

| | Per day |
|---|---|
| Inbound messages | about 1,190 |
| `[图片]` | 152–202 |
| `[动画表情]` | 59–83 |
| Total images | about 250 |

Plain images outnumber stickers about three to one, and people often post memes as plain images, so filtering on QQ's `sub_type` alone would miss many (a judgement; the mix is unmeasured). Every image needs looking at. Counts may include images inside forwards.

| Cost at repo prices | Per image | Per day at 250 |
|---|---|---|
| `gemini-3.8-flash`, one image, 100 output tokens | about ¥0.0006 | about ¥0.15 |
| Worst case, four frames each | about ¥0.0019 | about ¥0.5 |

Repeat memes cost nothing once their hash is known. Disk is not a limit: at an assumed 300 KB per image this is about 75 MB a day, and 1,390 GB is free.

## Pipeline

```
image arrives -> stored message, placeholder as today
              -> queue (bounded; overflow dropped and logged)
worker:  download -> sha256 -> known hash? add a sighting, stop
         normalize -> Flash: describe and judge -> save media record
         keep? -> save sticker -> BGE-M3 -> save vector
```

- **Never in the chat loop.** Relay stalls reached 111 s today on a call of about 370 tokens, and the chat loop handles one turn at a time. The worker runs one image at a time with a 120 s limit and no retry; the two live runs took about 34 and 45 s end to end, and a 30 s limit timed out once. An image whose description failed is tried again when it next appears.
- **Budget:** pauses when the daily budget is exhausted, and logs that it did.
- **Ignored groups:** never touched, as today. Read-only groups are collected; they are recorded already.
- **Unsupported formats:** TGS and WebM stay placeholders.
- **Model routing:** a new cheap `vision: true` scenario in `config/scenarios.yaml`, so it follows the existing router.

### What the model returns

```json
{"keep": true, "description": "猫瞪着屏幕，配字“你再说一遍”", "text": "你再说一遍",
 "use_when": "对方说了离谱的话", "avoid": "严肃话题", "tags": ["无语", "猫"],
 "scope": "any", "scope_reason": "通用梗图，没有群内信息"}
```

`scope` is `any` by default, or `origin` when the image only makes sense in its group, such as an in-joke or a group member's face. `keep: false` covers documents, anything with names or numbers, and images that only make sense in one conversation. Text inside an image is treated as quoted content, never as instructions.

## Storage

| Collection | Holds | Kept |
|---|---|---|
| `media` | message, chat, scope, sender, hash, description | Same as messages |
| files, not kept as stickers | Bytes | Deleted after 7 days |
| `stickers` | hash, file, description, use_when, avoid, tags, origin group, scope and its reason, sightings, status, sent_count, last_sent, Telegram `file_id` | Until the owner removes them |
| `sticker_embeddings` | hash, model, text hash, vector | Rebuildable |

Vectors stay in MongoDB and are compared in the bot's process, as `src/mind/embedding.rs` already does for episodes, with no vector server. MongoDB's own vector search is a Linux-only preview [1], and a separate vector service isn't justified for a table this size.

## Crossing groups

A kept sticker can be used in any group or private chat unless the model set `scope: origin` when collecting it, and the owner can change that in the web UI. This overrides the group boundary in [vision.md](vision.md) for kept stickers only. Descriptions in transcripts still stay in their own chat.

The accepted risk: there is no human step and no second check, so if the filter misjudges a private photo or chat screenshot as a generic sticker, he can send it to another group. Stage 1 sends nothing, so reviewing its results before stage 2 is the point at which to catch this.

In private chats, only what the owner sends is collected. Images from anyone else are described for that chat only, kept 7 days, and never appear in the web UI, and SelfOnly and LocalOnly material never goes to a cloud model, as now.

## Web UI

There is no approval step: stickers go live on collection, and the UI is how the owner reviews and corrects them. There are no chat commands.

| View or action | Effect |
|---|---|
| Grid | Thumbnails with description, scope, tags, sent count and origin group; animated stickers play |
| Filters | Status, scope, origin group, tag, free text, newest or most sent |
| Rejected | What the filter turned down in the last 7 days, while the files still exist, with the option to keep one |
| Edit | Description, use_when, avoid, tags, scope |
| Disable or delete | Disable keeps the hash, so it is never re-collected; delete removes the entry and file. Both work in bulk |
| Search test | Type a chat line, see the top 5 with scores, the main way to debug retrieval |

**Shape:** a subcommand of the bot's own exe, `aglibot stickers-web`, run as a separate process serving one embedded page on `127.0.0.1:8790`. Host and custom-header checks stop other sites from driving it.

- **Why not inside the bot:** the bot has no HTTP server today, and a page in the live process would stop at every restart and add a server to the chat process. A separate process stays usable while the bot is down or being deployed.
- **Why not Moldloft:** it tracks tickets and notes, and its contract keeps it to that.
- **Edits reach the bot through the database,** as the `lexicon` collection does, picked up on a short TTL. Editing text marks the vector stale through its text hash, and the bot's worker re-embeds. Only the search test calls Ollama directly.
- **Access:** localhost only and no login, so it works on this machine alone. Reaching it from a phone needs authentication first and is out of scope.
- **Cost:** one new dependency (`axum`), and `ops/deploy.ps1` restarts the page after the bot.

## How he uses it

1. On an addressed turn, embed the recent lines.
2. Offer up to 5 usable entries above a threshold in the turn's tail, never in L1 or L2, so the cached prefix stays unchanged: `可选表情：[表情:ab12] 猫瞪着屏幕（对方说了离谱的话时）`.
3. He may write `[表情:ab12]` or nothing. The runtime checks the id against the offered set and applies per-chat limits.
4. No candidates, no list. If the embedder is down, fall back to tag matching and log it.

Offering 5 costs about 150 tokens per turn; listing 50 would cost about 1,500, replayed on every later turn of the session.

### Sending

| Channel | Path | State |
|---|---|---|
| QQ | Image segment with base64 content and `sub_type: 1` | From a third-party copy of NapCat's guide [2]; the exact `base64://` form is unverified. `reply_params` sends text and @ only today |
| Telegram | Upload once, then reuse the cached `file_id` [3]. GIFs would use `sendAnimation` | Not implemented |

A failed send drops the sticker and keeps the text. An unknown outcome must not retry.

## Embedder: BGE-M3 on this machine

Measured on 2026-11-06 with Ollama 0.40.15 from winget: BGE-M3 gives 1024-dimension vectors through `/v1/embeddings` and runs 100% on the RX 9070 XT through Ollama's bundled ROCm, despite the doubts in third-party reports [7]. A query takes 14–29 ms warm and about 4.7 s when the model has to load, which Ollama does after 5 idle minutes.

Use `127.0.0.1`, not `localhost`: `localhost` adds a steady 2 s per request here. `config/models.yaml` now uses `127.0.0.1`, and has its own `sticker_embedding` block, so this does not turn on semantic recall (#2).

A first check on five made-up stickers ranked the sensible one first for all five relevant lines, but an unrelated line ("明天几点开会？") scored 0.465, above a correct match at 0.454. So a similarity threshold alone won't decide when to offer nothing, and stage 2 will rely on him abstaining. Five items is a smoke test, not an evaluation.

Starting Ollama also matters for routing: a running server used to enable the `local` chat model even with that model not pulled. A local model now needs to appear in the server's model list.

## Stages

**Stage 1: collect silently.** Media cache, cache-first reads, descriptions in transcripts, pixel rule, `stickers`, embeddings, web UI. He still sends only text. This alone improves chiming in and later sessions.

**Stage 2: send.** Retrieval, marker, QQ and Telegram sending, limits. Start only after reviewing what stage 1 collected.

## Evaluation

Targets are proposals, not results.

| Stage | Measure | Target |
|---|---|---|
| 1 | Private photos or chat screenshots kept with `scope: any`, one week reviewed | none |
| 1 | Owner agrees with a sample of 100 keep and reject decisions | at least 80% |
| 1 | Same meme re-posted by another sender gives the same hash | measure; decides how well repeats are skipped |
| 1 | Worker delay, arrival to description, p95 | under 60 s |
| 1 | Effect on chat latency | none measurable |
| 2 | Of 30 labelled chat snippets, an acceptable sticker is in the top 5 | at least 80% |
| 2 | Snippets that should get nothing get no list, or he abstains | at least 90% |
| 2 | Stickers sent in serious contexts (grief, conflict, safety) | none |

## Risks

- **Filter mistakes:** the main risk, covered under crossing groups.
- **More goes to Google:** every image in every collected group now goes to the Gemini relay, not only the ones he was addressed about, and group members don't know. The owner accepted this.
- **Overuse:** a sticker on every turn is grating. Limits need tuning, and sending should be opt-in per group.
- **Junk growth:** kept stickers accumulate. `sent_count` and a periodic review can prune them later.
- **Prompt injection:** text in images enters descriptions, so descriptions stay quoted data in every prompt.

## Unverified

Filter accuracy; description accuracy on memes; retrieval quality on Chinese memes; whether QQ keeps file bytes across re-sends; the real sticker and meme mix; NapCat sticker sending on the deployed version.

## Sources

All accessed 2026-11-06.

1. [MongoDB Community search and vector search preview](https://www.mongodb.com/products/updates/public-preview-mongodb-community-edition-now-offers-native-full-text-and-vector-search/).
2. [NapCat message handling, third-party copy](https://www.mintlify.com/NapNeko/NapCatQQ/guides/message-handling).
3. [Telegram Bot API](https://core.telegram.org/bots/api).
4. [BGE-M3 specification](https://inference.readthedocs.io/en/latest/models/builtin/embedding/bge-m3.html).
5. [Ollama embedding models guide](https://www.morphllm.com/ollama-embedding-models), third-party.
6. [Ollama GPU documentation](https://docs.ollama.com/gpu.md).
7. [ollama-rocm](https://github.com/xnyzer/ollama-rocm), unofficial.

## Decision needed from owner

- After reviewing a week of stage 1, authorize stage 2 or not.
