# Gate #58: memes and stickers research

2026-11-02. Research only; development is not authorized. Providers, interfaces, limits and targets are recommendations. External facts use numbered sources, all accessed **2026-11-02**. No live messages, account probes or recognition benchmarks were performed.

Follow-up, 2026-11-06: [sticker-store.md](sticker-store.md) covers an owner-managed sticker catalogue with vector search, and notes what has changed since this baseline.

## Repository baseline

`src/channels/qq.rs::parse_event` replaces `face` with `[表情]`, `image` with `[图片]`, and `record` with `[语音]`; identifiers and URLs disappear. Unknown segments, including standalone `mface`, are ignored; an otherwise empty message is dropped. `telegram.rs::parse_message` requires `text`, so sticker-only and voice messages are skipped. Both outgoing `send` methods send text only; QQ deliberately does not interpret CQ codes in model output.

Reuse [vision.md](vision.md)'s proposed question-conditioned description/OCR, bounded retrieval and uncertainty handling, not an assumed implemented vision feature. `config/models.yaml` lists Bailian Token Plan, DeepSeek, Anthropic and local Ollama, but no sticker capability. Apply `docs/DESIGN.md` §§2b/5 and `src/ns.rs` independently to disclosure Scope and ProcessingPolicy. No dependency on the unapproved persona work in #51/#52.

## Payloads, formats and reusable identifiers

| Channel/item | Verified representation and proposed treatment |
|---|---|
| QQ native face | OneBot `face.data.id` identifies a QQ face, on receive and send [1]. Keep the ID and a versioned, owner-reviewed description; never invent an ID from a semantic label. |
| QQ market face | NapCat documents market faces arriving as `image` segments distinguished by subtype; sending supports `mface`/`image`. Its `mface` example uses `emoji_id`, `emoji_package_id`, `key`, and optional `summary` [2]. Preserve these only when actually present; do not assume every received image exposes an outgoing tuple. |
| QQ image sticker | NapCat image fields include `file`, `sub_type`, `summary`, URL and file identifiers [2]. Preserve segment order and surrounding text. Resolve bytes through the channel; do not treat a URL, filename or hash as a permanent portable asset ID. |
| Telegram sticker | `Sticker` supplies `file_id`, `file_unique_id`, format flags, optional emoji and set name. `getStickerSet(name)` retrieves set metadata. `sendSticker` supports WebP, TGS and WebM. Reuse `file_id` within the same bot; it is not transferable between bots. `file_unique_id` is for identity, not download/send [3]. |

Telegram's creation requirements distinguish static PNG/WebP (one side 512 pixels, other at most 512), Lottie-based TGS (512×512, at most 3 seconds/64 KB, 60 FPS), and WebM/VP9 video (one side 512, at most 3 seconds/256 KB/30 FPS, no audio) [4]. These are asset requirements, not evidence that our current channel can decode them.

Telegram hosted `getFile` downloads cap at 20 MB; links last at least one hour [3]. NapCat documents expired-image URL refresh and host-local file handling [5]. Reuse vision's authenticated resolver/proxy approach: never expose token-bearing URLs to models or treat a remote NapCat path as a local Windows path. QQ subtype meanings, ID stability, cache lifetime and deployed upload limits need captured fixtures; no universal numbers are asserted.

## Recommended v1 and non-goals

**Recommend recognition plus curated selection. Generation remains undecided and excluded from this proposal.** Start with an owner-reviewed catalogue of approximately 20–40 assets: greetings, thanks, acknowledgement, mild amusement and encouragement. Outgoing candidates: QQ native faces and rights-cleared image stickers; Telegram static stickers reused by `file_id`. Market-face sends remain disabled until account permission and tuple round-trip checks pass.

Incoming known catalogue assets get a description without new image inference. Unknown static images use vision's proposed caption/OCR route only when requested and policy allows: JPEG/PNG/static WebP, 5 MiB encoded, 20 megapixels decoded, bounded normalization. An unknown TGS/video/animated image remains an explicitly unsupported-motion placeholder; a thumbnail may be described as a preview, never as the whole animation. Keep metadata so a future decoder does not require another ingestion redesign.

No pack scraping, automatic catalogue promotion, cross-platform republishing, set creation, custom emoji/masks, animation rendering, face identification, celebrity impersonation or image generation. Known animated assets may be described from reviewed catalogue metadata, but sending them is deferred.

## Contextual interpretation and selection

Treat visual observations, literal text and inferred intent separately. A smiling face might be friendly or sarcastic; crying can be playful exaggeration or distress; a thumbs-up can acknowledge or dismiss. These are ambiguity fixtures, not universal cultural mappings. Emoji annotations, set names, summaries and OCR are untrusted hints. Do not infer a person's mood, relationship or consent from one sticker.

The model chooses text, a permitted asset, or abstention using conversation and audience context; runtime enforces ACL, floor, policy, catalogue and budget constraints. Proposed default: no unsolicited stickers to strangers or groups. Require explicit group invocation and group opt-in; permit at most one sticker per reply. Prefer text for grief, conflict, health/safety matters, consequential instructions or uncertain irony. Avoid romantic, sexual, insulting and political assets in the starter catalogue. A sticker must not replace a substantive answer or acknowledgement of distress.

## Interface sketch and privacy

Illustrative only:

`StickerRef {platform, account, chat, message, ordinal, kind, opaque_locator, ns, scope, policy}`

`interpret(ref, question, context)` → `{observations, literal_text, possible_intents, uncertainty, provenance, usage}`

`choose(context, permitted_catalogue)` → `{text | asset_id | abstain, reason}` → runtime validation → channel send → `{sent | unknown | failed, message_id}`.

Catalogue entries should include internal asset ID, platform locator, content hash/version where available, reviewed description, permitted contexts, rights evidence/attribution, and revocation state. Separate reusable public artwork from private conversational associations. Never expose raw locators as model-callable URLs; validate model-selected IDs against the offered set. Do not retry an unknown send outcome blindly.

Inherited Scope/ProcessingPolicy follows pixels, OCR, interpretations, caches and memories. Private associations never become public catalogue metadata automatically. SelfOnly is not owner-disclosable; LocalOnly cannot fall back to cloud vision or cloud interpretation. Propose immediate deletion of fetched bytes and one-hour orphan cleanup; retain approved catalogue assets separately. Derived descriptions follow the owner-selected message retention period.

## Permissions, risks and evaluation

Telegram explicitly accepts copyright complaints about public sticker sets [6]. Neither a working platform ID nor possession of bytes is accepted here as rights evidence. Proposed catalogue policy: owner-created or explicitly licensed assets, record attribution and redistribution conditions, and revoke promptly when permission changes. QQ paid-pack automation, cross-platform rights and rights to individual candidate assets remain **unverified**; do not bypass purchases or assume an open-source bridge licenses artwork.

Main risks: cultural misreading, humiliating private images, malicious OCR, expired locators, unsupported animation, resource exhaustion, account-specific market permissions and unintended group disclosure. Cloud image retention/training/deletion terms remain unverified as in vision research.

Proposed 20 owner-labelled fixtures: known/unknown QQ face; market face as image; standalone mface; ordinary image versus sticker; repeated/expired locator; TG static/TGS/video; missing set/emoji; same unique ID across bots; Chinese pun; sarcastic smile; playful versus distressed crying; stranger greeting; serious group discussion; private-to-group leak; LocalOnly; OCR injection; revoked asset. Include paired contexts for ambiguous images.

Targets, not results: 100% identifier/order preservation on supported fixtures; at least 90% required observations on readable static images; at least 90% owner agreement on selection/abstention; every ambiguous case explicitly qualified; zero unsafe-context sends, fabricated IDs or policy leaks. Measure cold start and three warm repeats: provisional p95 selection ≤2 seconds without vision, ≤10 seconds with cloud vision, ≤30 seconds locally. Report recognition cost separately from downstream chat. These small fixtures are a smoke gate, not statistical assurance.

Unverified: deployed QQ payload variants, limits, market permissions and locator longevity; candidate artwork rights; account vision access and charges; cloud retention; local decoder/model readiness; contextual accuracy and latency targets.

## Sources

All accessed 2026-11-02.

1. [OneBot v11 segments](https://github.com/botuniverse/onebot-11/blob/master/message/segment.md).
2. [NapCat message formats](https://napneko.github.io/develop/msg).
3. [Telegram Bot API](https://core.telegram.org/bots/api).
4. [Telegram sticker requirements](https://core.telegram.org/stickers).
5. [NapCat file handling](https://napneko.github.io/develop/file).
6. [Telegram copyright FAQ](https://telegram.org/faq#q-a-bot-or-channel-is-infringing-on-my-copyright-what-do-i-do).

## Decision needed from owner

- Keep research-only or separately authorize development; approve recognition plus curated selection.
- Approve static-first scope, market-face deferral, group opt-in and acceptance targets.
- Choose rights-cleared starter assets and permitted contexts; decide derived-description retention.
- Select vision processing policy/account; leave generation explicitly undecided.
