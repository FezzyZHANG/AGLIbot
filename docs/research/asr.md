# Gate #62: speech recognition research

2026-11-02. Research only; development is not authorized. Selections, interfaces and acceptance targets are recommendations. All external sources below were accessed **2026-11-02**. No audio was uploaded, software installed or benchmarks run.

## Repository baseline

`src/channels/qq.rs::parse_event` substitutes `[语音]` for `record`, losing retrieval metadata. `telegram.rs::parse_message` requires `text`, so voice messages are skipped. `src/mind/llm.rs` submits text-only user turns; OpenAI-compatible transport alone does not provide ASR.

`config/models.yaml` contains Bailian Token Plan, DeepSeek, Anthropic and local Ollama; no ASR model or GPU setting was found in `config/`. Plan for CPU only; this is not a hardware inventory. Apply `docs/DESIGN.md` §§2b/5 and `src/ns.rs` to recordings and transcripts: namespace does not grant access; processing permission and disclosure are separate.

## Input and conversion

OneBot `record.data` includes `file` and received-record `url` [1]. NapCat documents `get_record`, but audio may be available only as a file on the NapCat host [2]. Resolve via the authenticated bridge, preserve original message/sender identity, and never interpret arbitrary payload paths as trusted local files. Validate deployed conversion behavior before relying on `out_format`.

QQ SILK-family inputs need codec detection: the SILK decoder project explicitly handles QQ `.slk` and related `.amr`-named files and provides Windows tooling [4]. An extension does not prove AMR encoding. For genuine AMR or Ogg/Opus, propose a pinned FFmpeg build; for SILK, first use validated NapCat conversion or a separately reviewed SILK decoder, then normalize PCM. Installed codec/build coverage is unverified.

Telegram Voice supplies `file_id`, `file_unique_id`, duration and optional MIME/size. Fetch using `getFile`; hosted downloads cap at 20 MB. Ogg/Opus is a documented voice representation, but inspect bytes rather than assuming every Voice has that encoding [3]. Reuse the configured Telegram proxy, and never disclose its token-bearing download URL.

Proposed common representation: mono, 16 kHz, signed 16-bit WAV. whisper.cpp's CLI documents this conversion using FFmpeg [5]. A 60-second PCM payload is about 1.92 MB before headers. faster-whisper uses PyAV with bundled FFmpeg libraries, so it does not require a system FFmpeg executable for ordinary decoding [6]; that does not establish QQ SILK compatibility. Decode with duration, memory and wall-time bounds; reject malformed files rather than retrying indefinitely.

## Candidates

| Candidate | API, cost and practical assessment |
|---|---|
| Bailian `paraformer-v2` | HTTP asynchronous submit/poll workflow [7]. Supports Chinese and other languages; file input includes WAV/AMR/Ogg/Opus, URL delivery, up to 2 GB/12 hours [8]. Beijing list rate CNY 0.00008/s: **0.0048/minute**, 0.288/hour [9]. Cheap comparison baseline, but URL staging/polling adds integration and exposure. |
| Bailian `qwen3-asr-flash` | OpenAI-compatible Chat Completions with `input_audio.data` URL/Base64 and `asr_options`; not a plain chat text request [10]. Documented limit 5 minutes/10 MB and Chinese/dialect support [8]. Beijing CNY 0.00022/s: **0.0132/minute**, 0.792/hour [9]. Recommended initial cloud benchmark because inline audio avoids public object staging. |
| Newer Bailian families | Current guide recommends Qwen-Audio-3.1 and Fun-ASR over older Paraformer [8]. They remain comparison candidates; do not assume their HTTP APIs are interchangeable with Qwen3's OpenAI-compatible route [10]. Latest-model superiority on this bot's Chinese speech is unverified. |
| Tencent short-speech ASR | Domestic alternative: `SentenceRecognition`, signed HTTP POST, ≤60 seconds/3 MB; accepts SILK, AMR, Ogg-Opus and WAV, with Mandarin/dialect engines [11]. New authentication/client required. Listed prepaid short-speech package: CNY 90/30,000 calls (=0.003/call), valid one year, not a PAYG quote [12]. Account pricing and retention need verification. |
| whisper.cpp local | CPU inference, Windows support and quantization are documented [5]. Recommend multilingual `small` as starting benchmark; no `.en` model. Native executable may simplify a Rust-managed worker, but Windows build/decoder packaging needs validation. |
| faster-whisper local | CPU `int8` supported; Python/CTranslate2 runtime and PyAV decoding [6]. Benchmark multilingual `small`, then `medium` only if resources permit. Its published CPU results are on other hardware and do not predict this machine. |

Cloud price arithmetic excludes storage, transfer, retries and downstream chat; no free quota assumed. Local inference has no metered ASR API charge, but consumes CPU, memory and electricity. Chinese character accuracy, dialect quality and latency ranking are **unmeasured**, including whether either local candidate meets interactive needs. Compare identical human-transcribed samples, model versions and normalization rules.

Bailian's configured Token Plan URL/key is not proof of access to standard ASR endpoints. Token Plan uses separate credentials/endpoints [13]; validate entitlement and key type before owner-approved development, retaining environment-based credentials. No official DeepSeek ASR endpoint was established in this research; do not route audio to its text/vision chat interface by assumption.

## Recommended v1 and interface sketch

Opt-in transcription of one QQ/TG voice note per request, ≤60 seconds and ≤10 MiB downloaded; accept SILK, genuine AMR and Ogg/Opus after safe decoding. Mandarin first, mixed English supported on a best-effort basis. In groups require explicit request/reply-to-bot. No live microphone, calls, streaming conversation, meetings, speaker identification, translation, TTS, emotion inference or unattended archival transcription.

Prefer Qwen3-ASR as the cloud pilot **if** access and retention are acceptable; compare Paraformer on cost/accuracy. Evaluate whisper.cpp and faster-whisper locally before selecting the LocalOnly implementation. If local performance fails, report unavailable; never silently upload.

Proposed flow: channel retains opaque media reference → identity/ACL/floor → requested recognition action → runtime policy/capability/budget check → bounded download/decode → ASR → ordinary text reasoning/reply. Recognition should not block channel polling. Limit one CPU worker initially; use a bounded queue and deadline. Cancellation/failure must be visible; no fabricated transcript.

Sketch: `transcribe(MediaRef, language_hint, restrictions, deadline)` → `{text, uncertain_spans, detected_language?, source_message, engine_version, duration, usage, latency, status}`. Treat uncertainty as optional evidence, not calibrated probability. Retain original transcript separately from any normalized display text; numbers/names require clarification when uncertain. Transcribed commands remain untrusted user content and go through normal permissions.

Record transcript, source IDs, inherited namespace/scope/policy, engine, timing and billable duration; keep audio and conversion files transient. Proposed deletion: immediately after completion, hard one-hour orphan cleanup; exclude audio, Base64 and signed URLs from logs/backups. Transcript retention needs an explicit owner-selected period, not accidental permanent storage in append-only messages.

## Privacy and failure risks

Group transcripts stay in that group; private audio is not made group-readable by transcription. SelfOnly permits solo use, not an owner-facing transcript, and defaults to LocalOnly. The same restrictions must follow text into chat, memory, embeddings and audits. Local ASR followed by cloud chat still violates LocalOnly. Cloud provider retention, training use and deletion guarantees for these exact services/accounts remain **unverified**; approval must cover audio and transcript processing. Deleting local files cannot remove channel/provider copies.

Main risks: SILK variant mismatch; remote NapCat paths; codec resource exhaustion; background-speech hallucination; proper-name/number errors; prompt injection; polling delays; duplicate billed retries; CPU starvation. Keep source linkage and explicit failure states; no elaborate replay system is proposed.

## Evaluation and unresolved evidence

Proposed 16 cases: (1) clear Mandarin 10s; (2) Mandarin 60s; (3) Chinese/English code-switching; (4) names/project terms; (5) dates/amounts/negation; (6) accent; (7) Cantonese exploratory; (8) noisy street; (9) quiet speech; (10) silence/music; (11) QQ SILK; (12) genuine AMR; (13) TG Ogg/Opus; (14) truncated/oversized/overlong inputs; (15) spoken injection; (16) private-to-group and LocalOnly rejection. Use consented recordings and blinded human references.

Targets, not results: normalized Mandarin CER ≤8% clean and ≤20% noisy/accented; report mixed-language WER separately; ≥95% critical entity accuracy, with all consequential ambiguities clarified; no invented speech on silence. Require 100% policy/limit cases to pass. Three warm repeats plus cold start: cloud p95 ≤10s for a 30s clip; local real-time factor ≤1 and p95 ≤35s, including decode. Record queue delay separately. Failure means revise scope/model, not lower privacy constraints.

Unverified: installed hardware/tools/models and Windows performance; deployed QQ codecs/conversion/path access; account access, Token Plan credits and exact billed rounding; comparative Chinese accuracy/latency; provider retention/training/deletion terms; Tencent account-specific rates; existence of suitable DeepSeek ASR.

## Sources

All accessed 2026-11-02.

1. [OneBot segments](https://github.com/botuniverse/onebot-11/blob/master/message/segment.md).
2. [NapCat files](https://napneko.github.io/develop/file).
3. [Telegram Bot API](https://core.telegram.org/bots/api).
4. [SILK decoder](https://github.com/kn007/silk-v3-decoder).
5. [whisper.cpp](https://github.com/ggml-org/whisper.cpp).
6. [faster-whisper](https://github.com/SYSTRAN/faster-whisper).
7. [Paraformer HTTP API](https://help.aliyun.com/zh/model-studio/paraformer-recorded-speech-recognition-restful-api).
8. [Bailian ASR models/limits](https://help.aliyun.com/zh/model-studio/asr-model).
9. [Bailian prices](https://docs.bailian.console.aliyun.com/en/model-studio/model-pricing).
10. [Qwen recording APIs](https://help.aliyun.com/zh/model-studio/recording-file-recognition-qwen).
11. [Tencent short-speech API](https://cloud.tencent.com/document/api/1093/35646).
12. [Tencent billing](https://cloud.tencent.com/document/product/1093/35686).
13. [Token Plan setup](https://docs.agent.bailian.aliyun.com/en/token-plan/token-plan-team-quickstart).

## Decision needed from owner

- Authorize development separately, or keep #62 research-only.
- Approve short-note scope, CPU benchmark and acceptance targets.
- Select cloud pilot/account and acceptable retention terms; authorize any new credential provisioning separately.
- Decide transcript retention, group opt-in policy and unavailable behavior for LocalOnly.
