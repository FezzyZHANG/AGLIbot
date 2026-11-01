# Gate #66: speech synthesis research

2026-11-02. Research only; development is not authorized. Models, interfaces and targets are recommendations. External sources were accessed **2026-11-02**. No synthesis calls, uploads, installations or benchmarks were run.

## Repository and machine baseline

QQ currently replaces incoming `record` with `[语音]`; Telegram requires `text` and skips voice messages. Both outgoing channel methods send text only. `config/models.yaml` contains Bailian Token Plan, DeepSeek, Anthropic and Ollama chat routing, not TTS configuration. Existing credentials/HTTP compatibility do not establish synthesis entitlement. Preserve environment-based credentials; no dependency on persona tickets #51/#52.

Reuse [asr.md](asr.md)'s codec detection, bounded conversion, remote NapCat path caution and Telegram proxy handling. Its 16 kHz ASR normalization is not a required TTS output rate. Read-only checks on this Windows session found Python at `C:\Python314\python.exe`, but no `ffmpeg`, `ffprobe`, `nvidia-smi` or `sherpa-onnx-offline-tts` on PATH. CIM hardware queries returned access denied. This does not prove software/GPU absence: CPU/RAM/GPU, installed models and throughput remain unverified; plan a CPU baseline.

## Candidates and economics

Domestic service endpoints are documented for Bailian Beijing and Tencent [3,5]; actual connectivity, account access and latency from this machine remain untested. No pronunciation or quality ranking below is a measured result.

| Candidate | Evidence and proposed fit |
|---|---|
| Bailian CosyVoice | `cosyvoice-v3-flash` offers system voices with Mandarin/dialect support depending on voice; `v3-plus` system voices cover Mandarin/English. Current `v3.5` has no system voices, so exclude it from this preset-only v1 [1]. Benchmark v3-flash for neutral conversational delivery. HTTP supports full-text synthesis, streaming/nonstreaming output, and WAV/PCM/MP3/Opus [3]. Beijing rates: v3-flash CNY 1, v3-plus CNY 2 per 10,000 billable characters [2]. |
| Bailian Qwen-TTS | `qwen3-tts-flash` has system voices covering Chinese and dialects, voice-dependent [1]; Beijing CNY 0.8/10,000 characters [2]. Recommended first cloud comparison against CosyVoice. Newer `qwen-audio-3.1-tts-plus` costs CNY 1.4/10,000; its Flash sibling uses token billing, so do not compare their names as equivalent tariffs [2]. Choose a neutral preset after listening, not from marketing descriptions. |
| Tencent TextToVoice | Chinese/English selection, voice IDs, SSML and WAV output are documented [5]. Alternative domestic provider requiring its own signed API integration. General synthesis PAYG: boutique voices CNY 0.3/10,000; large-model voices CNY 1.2/10,000 at the lowest daily-volume tier; supernatural 2.0 CNY 2.5/10,000 [6]. Compare a Mandarin preset for numbers/names and conversational suitability. |
| Local sherpa-onnx + Matcha | Runtime supports offline synthesis and Windows; Apache-2.0 repository license [7]. Its TTS documentation lists Chinese `matcha-icefall-zh-baker` [8]. First CPU benchmark candidate for fixed-voice reading. Runtime licensing does not establish the selected weights/dataset/voice redistribution rights; those remain unverified. |
| Local Qwen3-TTS | Repository lists 0.6B/1.7B CustomVoice models with nine preset timbres and Chinese support, under an Apache-2.0 repository license [9]. “CustomVoice” here includes presets, not a requirement to clone. Benchmark 0.6B presets only after resource/weight-license checks; documented examples use CUDA. Windows compatibility and acceptable CPU speed on this machine are unverified. |

Bailian's published counting rule counts Han characters as two billing characters [4]; Tencent counts Chinese characters, letters, punctuation and spaces individually [6]. For 100 Han characters without punctuation: illustrative Qwen3 Flash **CNY 0.016**, CosyVoice v3 Flash **0.020**, Tencent boutique **0.003**. Different quality tiers make this arithmetic no quality ranking. Exclude retries, transfer and downstream chat; no free quota or Token Plan coverage assumed. Local execution avoids metered synthesis calls but uses compute/electricity; actual cost is unknown.

## Outgoing codecs and limits

QQ OneBot/NapCat sends `record` with `data.file` [10]. Retain ASR research's QQ SILK compatibility concern: NapCat's current helper detects SILK and exposes PCM conversion [11], but that alone proves neither outbound encoding nor a working deployed conversion path. Proposed output pipeline: provider WAV/PCM → validated QQ-compatible SILK encoder or verified bridge conversion → `record`. **Unverified:** deployed SILK header/sample-rate requirements, automatic transcoding, maximum bytes/duration and encoder redistribution license. Do not substitute Opus merely because it contains SILK-related technology. Require a device playback round trip before enabling QQ voice.

Telegram `sendVoice` accepts Ogg/Opus, MP3 or M4A, up to 50 MB [12]. Recommend Ogg/Opus for consistent voice-note delivery, uploaded through the configured proxy. A provider's “Opus” option alone does not establish an Ogg container: inspect/remux or encode from PCM, then verify playback. Proposed bot caps: 200 Unicode characters, 30 seconds and 2 MiB encoded per reply; these are deliberately independent of platform maxima. Preserve native synthesis rate until channel conversion; inspect actual duration and size before sending.

## Recommended v1, interface and modality

Opt-in short Mandarin voice notes with a stable, owner-approved preset; mixed English best effort. Text remains default. Choose voice for an explicit request or a saved recipient preference, only after audience/policy checks. Groups require explicit invocation and opt-in; strangers get text. Keep code, URLs, detailed instructions and consequential numbers in text; include a text equivalent with requested speech. Do not automatically answer voice input with voice output.

No cloning, custom training, voice design, celebrity imitation, singing, calls, live microphone or streaming conversation. The owner must separately approve cloning/training, including speaker consent, recording rights, allowed uses and revocation. No consent or licensing conclusion is established here for such use.

Proposed flow: finalized approved reply → model chooses modality → runtime validates ACL/floor, Scope, ProcessingPolicy, capability and budget → bounded synthesis job → codec validation → send. TTS receives the finalized spoken text, not conversation history. QCL estimates need synthesis-specific quality, completion latency and billing units; do not reuse chat token prices.

Sketch: `synthesize {text, preset_id, language, ns, scope, policy, audience, deadline}` → `{audio_ref, codec, container, sample_rate, duration, engine_version, usage, status}`; `send_voice {audio_ref, destination, text_equivalent}` → `{sent | unknown | failed, message_id}`. IDs are allowlisted; raw paths/URLs are not model-selected. Read numbers and abbreviations through an approved normalization layer; preserve original text and never silently rewrite meaning.

No user-facing streaming is needed for complete voice notes. Provider streaming may later reduce buffering, but adds little until delivery can start early. Support cancellation before send, bounded queues and deadlines; do not promise interruption of a note already delivered. Revalidate audience/policy before sending; record unknown send outcomes without blind retries. Failure returns text when disclosure permits; LocalOnly never falls back to cloud.

## Privacy, licensing and evaluation

Apply DESIGN §§2b/5 and `src/ns.rs` to text, audio, caches and logs. Persons scope stays private; group content stays in its group; SelfOnly is not owner-disclosable. Local synthesis does not authorize cloud processing elsewhere. Propose deleting temporary audio immediately after delivery/failure with one-hour orphan cleanup, retaining only scoped text, usage and send outcome under message retention policy.

Cloud-specific retention, training use, deletion windows and preset-output commercial rights remain **unverified** for these accounts. Obtain service-specific terms before selection; local deletion cannot remove recipient/platform/provider copies. Local runtime licenses are not blanket clearance for weights, voice likenesses or codec binaries. Clearly identify speech as synthesized and avoid impersonation.

Proposed 16 fixtures: greeting; calm reassurance; polyphones (“银行行长”, “重庆重量”); names; dates; currency/decimals; negation; phone digits; Chinese/English acronyms; punctuation; 200-character boundary; overlong output; QQ SILK playback; TG Ogg/Opus playback; cancellation/unknown send; private-to-group and LocalOnly rejection. Use identical texts, pinned versions and three blinded Mandarin listeners; compare preset suitability separately from intelligibility.

Targets, not results: ≥95% word intelligibility, 100% critical numbers/negation correct, median naturalness and suitability ≥4/5, and all policy/limit cases passing. Test 20 additional short replies with three warm repeats plus cold start: provisional cloud p95 ≤8 seconds and local ≤15 seconds from finalized text to send acknowledgement for a 10-second note. Measure synthesis, queue, encoding and transport separately; no claimed first-byte latency. Report billed units and failures, not just successful averages.

Unverified: machine resources/installed engines, Windows performance, China connectivity, account/Token Plan access, actual billing rounding, comparative Chinese quality, QQ codec/limits, cloud retention/output rights and local weight/voice/encoder licenses.

## Sources

All accessed 2026-11-02.

1. [Bailian TTS models and voices](https://docs.bailian.console.aliyun.com/zh/model-studio/tts-model).
2. [Bailian model prices](https://docs.bailian.console.aliyun.com/zh/model-studio/model-pricing).
3. [CosyVoice HTTP API](https://docs.bailian.console.aliyun.com/zh/model-studio/cosyvoice-tts-http-api).
4. [Bailian billing character rules](https://help.aliyun.com/zh/model-studio/model-pricing?userCode=r3yteowb).
5. [Tencent TextToVoice](https://cloud.tencent.com/document/api/1073/37995).
6. [Tencent synthesis billing](https://cloud.tencent.com/document/product/1073/34112).
7. [sherpa-onnx runtime](https://github.com/k2-fsa/sherpa-onnx).
8. [sherpa-onnx TTS models](https://k2-fsa.github.io/sherpa/onnx/c-api/html/tts.html).
9. [Qwen3-TTS models and license](https://github.com/QwenLM/Qwen3-TTS).
10. [NapCat message formats](https://napneko.github.io/develop/msg).
11. [NapCat audio helper source](https://raw.githubusercontent.com/NapNeko/NapCatQQ/main/packages/napcat-core/helper/ffmpeg/ffmpeg.ts).
12. [Telegram sendVoice](https://core.telegram.org/bots/api#sendvoice).

## Decision needed from owner

- Keep research-only or separately authorize development; approve preset-only short notes and targets.
- Approve Qwen3 Flash/CosyVoice v3 Flash comparison, Tencent alternative and local CPU benchmark.
- Select acceptable provider retention/output terms and local model/codec licensing evidence.
- Choose the preset, group opt-in and retention policy; keep cloning/training outside v1.
