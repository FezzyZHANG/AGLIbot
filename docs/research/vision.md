# Gate #54: image recognition research

2026-11-02. Research only; development is not authorized. All interfaces, limits and selections below are proposals. External facts use numbered sources; every source was accessed **2026-11-02**. No account probes or recognition benchmarks were run.

## Implementation status

2026-11-06: the owner authorized development; the code is `src/mind/vision.rs`, which supersedes this document where they differ. The owner chose **direct image input to the chat model** over the caption-first path recommended below:

- **Model:** `chat` has `vision: true` and is pinned to `gemini-4-argon-medium`; a pin or route must be marked `vision: true` in `config/models.yaml`. The five Gemini relay models passed an image probe and are the only ones marked, so the repo default now needs the relay to start. Other providers are unmarked because they are untested, not because they are known to lack vision.
- **Formats:** JPEG, PNG, WebP and GIF are all normalized to JPEG. The relay rejects BMP and GIF outright. Animated GIF and WebP send up to four evenly spaced frames, overriding the rejection of animated input proposed below.
- **Limits:** 10 MiB per file and 50 megapixels, raised from the 5 MiB and 20 megapixels proposed below so ordinary phone photos fit. Longest edge 2048 pixels (1024 for frames), at most 8 images per turn and 4 waiting per chat for 10 minutes.
- **When:** only on a turn where he is addressed. Chiming in never inspects an image. An image in a quoted message counts, labelled with who sent it; images inside merged forwards do not.
- **Cost:** images stay in the session and replay on later turns, about 1,091 tokens each, so they benefit from prompt caching only where the relay provides it. A session rotates before passing 16 images.
- **Failure:** if the model rejects a turn with images, the turn is retried once without them and says they were not seen.
- **Retention:** locators are dropped after fetching, and pixels live only in the in-memory session. Nothing about an image's content is written to the message store, so a later session sees `[图片]` again. This is the conservative default for the open retention decision.

Relay probes on 2026-11-06 used the 512×512 Lena image and synthetic images, one run per case, so they establish cost and format handling but not accuracy. Fixtures and targets below remain unmeasured (#56).

## Repository baseline

`src/channels/qq.rs::parse_event` reduces images to `[图片]`, discarding segment data. `telegram.rs::parse_message` requires `text`: photo-only and captioned photo messages are skipped, not replaced with placeholders. `src/mind/llm.rs` has `Turn::User(String)` and sends string content. Existing OpenAI-compatible HTTP transport therefore helps, but cannot currently submit images.

`config/models.yaml` configures DeepSeek, Bailian Token Plan (with an environment URL override), Anthropic and local Ollama `qwen3:8b`. Its prices are routing estimates, not verified quotations. `docs/DESIGN.md` §§2b/5 and `src/ns.rs` distinguish namespace, disclosure Scope and ProcessingPolicy; extend those constraints to media and derived text before applying QCL selection.

## Retrieval and bounds

OneBot v11 image segments carry `data.file` and received-image `data.url` [1]. Preserve segment order and captions. NapCat supports `get_image`; its guide describes refreshing expired image URLs and warns about local versus remote file access [2]. Fetch through a trusted channel resolver, not a model-selected URL. A NapCat path belongs to its host, not necessarily this Windows worktree. Deployed version, URL lifetime and actual QQ byte limits remain unverified.

Telegram exposes `photo[]` size variants, `document`, `caption`, and `media_group_id`; resolve `file_id` through `getFile`. `file_unique_id` cannot download files. Hosted Bot API downloads cap at 20 MB; generated links last at least one hour [3]. Select one suitable photo size rather than treating variants as multiple images. Proposed album collection: same chat/group identifier, bounded two-second window, preserve message order; label late items separately. Download via the configured Telegram proxy; never send token-bearing Telegram URLs to a model.

Proposed common limits: four images/request; JPEG, PNG and static WebP only; 5 MiB encoded-file cap each, 20-megapixel decoded cap, 8 MiB combined normalized bytes, 2048-pixel longest edge. Reject unsupported/animated inputs explicitly. These are bot limits, not platform claims; inspect bytes and enforce limits during decoding. Strip EXIF and normalize orientation. Tiny text may require a user-supplied crop.

## Approaches

| Approach | Assessment for this bot |
|---|---|
| Direct image + question to chat | Preserves task-specific detail and cross-image reasoning; one inference can answer directly. Requires capability-aware routing, multimodal history and repeated-image cost control. Higher integration scope. |
| Question-conditioned caption + extracted text, then ordinary chat | Recommended v1. Keeps downstream text pipeline; separate observations, text and uncertainty. Adds a serial inference and risks omitting detail, so retain image indices and request clarification rather than inventing answers. |
| OCR only | Good baseline for screenshots and receipts; cannot explain scenes, memes or spatial meaning. Evaluate local OCR later; do not equate transcription with understanding. |

These are engineering assessments, not measured accuracy or latency rankings.

## Provider candidates and economics

| Candidate | Verified capability and integration implication |
|---|---|
| Bailian | Current vision guide names `qwen3.8-flash`, `qwen3.7-plus` and Qwen-VL options [4]. Recommend evaluating the configured Flash first if the account exposes vision; use `qwen3-vl-plus` as a comparison. OpenAI-compatible image input supports Base64; Qwen3-VL allows 20 MB raw images, but encoded Data URI is also capped at 20 MB. Base64 count limit is 250; request-size limits depend on endpoint [5]. Our smaller caps apply. |
| DeepSeek direct | `deepseek-flash` supports OpenAI-compatible `image_url` blocks, JPEG/PNG/GIF/WebP, up to 32 MiB inline per image, 48 MiB request body and 600 images; each image contributes at most 1024 tokens [6]. Pricing lists vision for Flash, not Pro [7]. Do not infer that configured `deepseek-chat` or Bailian-hosted aliases behave identically. |
| Local Ollama | `/v1/chat/completions` supports image content [8]; `qwen3-vl` offers 2b/4b/8b variants [9]. Recommend benchmarking 4b locally; configured `qwen3:8b` is not evidence of installed vision capability. Require genuinely local weights, no cloud-backed model. |

Token Plan has its own keys, endpoints and model entitlements [10]; standard Bailian pricing is **not** a quotation for this subscription. Account availability and credit consumption remain unverified.

For standard Beijing `qwen3-vl-plus`, the ≤32K tier is CNY 1/M input and 10/M output [11]. Illustrative 1,000 image tokens + 200 prompt + 200 output tokens costs **CNY 0.0032/image**, excluding downstream chat; 1,000 is an assumption, not a fixed image tariff. DeepSeek Flash peak uncached input/output is $0.30/$1.20 per million [7]: 1024 image + 200 prompt + 200 output tokens yields **$0.0006072/image**. Local inference avoids a metered cloud call but uses electricity/RAM/CPU; actual cost and latency are unmeasured. Record total recognition-plus-chat usage, not just the caption call.

## Proposed v1 and interface

Explicitly requested image descriptions, simple comparisons and readable screenshot text in QQ/TG; no passive group scanning. No video, face identification, biometric inference, document ingestion, exhaustive OCR, image generation or guaranteed exact counting.

Proposed placement: parse media references → identity/ACL/floor → model chooses whether to inspect → runtime checks scope, processing policy, capability and budget → bounded retrieval/normalization → recognition → ordinary appraise/decide/speak. Preserve a media placeholder until inspection; do not send unapproved pixels to triage.

Sketch: `MediaRef {platform, chat, message, ordinal, opaque_locator, ns, scope, policy}` → `describe(question, images, constraints)` → `{observations[], extracted_text[], uncertainties[], model, usage, latency, status}`. Names are illustrative, not approved APIs. Treat recognized text as untrusted quoted evidence, never system instructions. Missing capability or policy rejection produces an explicit unavailable status, never cloud fallback from LocalOnly.

Store source message/ordinal, derived text, restrictions, provenance and usage; do not persist Base64, credential URLs or raw model requests. Proposed raw-file lifetime: delete after processing, hard one-hour cleanup after crashes; persist derived text under the message retention policy, which needs an owner decision. Caches must include scope/policy boundaries.

## Privacy, risks and evaluation

Group-scoped descriptions may enter only that group's audience, never another group or private conversation automatically. Persons scope stays private. SelfOnly permits solo processing, not disclosure even to the owner; its default is LocalOnly (`src/ns.rs`). Local OCR/captioning does not authorize sending its output to cloud. Apply restrictions to downstream chat, embeddings, logs and memories. No verified provider-specific retention/deletion guarantee was established: cloud approval must cover media, derived text and provider logs; local cleanup cannot erase QQ/TG or provider copies.

Proposed 16 fixtures: (1) ordinary object; (2) Chinese screenshot; (3) mixed Chinese/English; (4) receipt amounts; (5) tiny text; (6) rotated photo; (7) ambiguous meme; (8) chart; (9) two-image comparison; (10) four-image ordering; (11) TG album/late item; (12) expired QQ URL; (13) oversized/corrupt file; (14) image prompt injection; (15) private-to-group leak; (16) SelfOnly/LocalOnly with cloud-only capability.

Owner-labelled targets: ≥90% required facts on readable fixtures; ≥95% character accuracy on clear printed text; zero invented critical amounts; 100% policy/limit/injection cases safe. Measure three warm repeats plus cold start: provisional cloud p95 ≤10 seconds to caption; local p95 ≤30 seconds. Targets are not measured results. Track failures, total reply latency and cost; 16 cases are a smoke gate, not statistical proof.

Unverified: account/model entitlements and aliases; QQ deployment limits/cache access; local installation/resources/throughput; comparative accuracy/latency; cloud retention/training/deletion terms; actual image-token and Token Plan charges.

## Sources

All accessed 2026-11-02.

1. [OneBot segments](https://github.com/botuniverse/onebot-11/blob/master/message/segment.md).
2. [NapCat file handling](https://napneko.github.io/develop/file).
3. [Telegram Bot API](https://core.telegram.org/bots/api).
4. [Bailian vision overview](https://docs.bailian.console.aliyun.com/zh/model-studio/vision-model).
5. [Bailian image API/limits](https://docs.bailian.console.aliyun.com/zh/model-studio/vision).
6. [DeepSeek vision](https://api-docs.deepseek.com/guides/vision/).
7. [DeepSeek pricing](https://api-docs.deepseek.com/quick_start/pricing/).
8. [Ollama compatibility](https://docs.ollama.com/api/openai-compatibility).
9. [Ollama Qwen3-VL](https://ollama.com/library/qwen3-vl).
10. [Token Plan setup](https://docs.agent.bailian.aliyun.com/en/token-plan/token-plan-team-quickstart).
11. [Bailian pricing](https://docs.bailian.console.aliyun.com/en/model-studio/model-pricing).

## Decision needed from owner

- Authorize development separately, or keep #54 research-only.
- Approve caption-first scope, limits and acceptance targets.
- Choose initial cloud candidate/account and acceptable retention terms; approve local benchmark candidate.
- Decide derived-text retention and explicit group invocation policy.
