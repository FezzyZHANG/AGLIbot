---
id: n-agli-idea-deepseek-app-integration
graph_id: default
version: 1
title: Candidate DeepSeek integrations and acceptance plan for AGLIbot
kind: note
state: active
in_graph: true
summary: '先对照 DSH 验证 Search/Fetch 的可追溯证据，再试带引用问答。文件解析/OCR、语音识别与合成复用现有评测工单，另列拒答、会话元信息、成本和取消的验收条件。


  Research comparison first. Evaluate document and voice features under their existing
  tickets, with distinct evidence, billing and processor boundaries.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-dec-deepseek-paid-api
- relation: relates
  target_type: note
  target_id: n-src-deepseek-app-api-20261113
- relation: relates
  target_type: ticket
  target_id: 100
- relation: relates
  target_type: ticket
  target_id: 101
- relation: relates
  target_type: ticket
  target_id: 102
- relation: relates
  target_type: ticket
  target_id: 21
- relation: relates
  target_type: ticket
  target_id: 25
- relation: relates
  target_type: ticket
  target_id: 44
- relation: relates
  target_type: ticket
  target_id: 55
- relation: relates
  target_type: ticket
  target_id: 56
- relation: relates
  target_type: ticket
  target_id: 62
- relation: relates
  target_type: ticket
  target_id: 64
- relation: relates
  target_type: ticket
  target_id: 66
- relation: relates
  target_type: ticket
  target_id: 68
- relation: relates
  target_type: ticket
  target_id: 82
- relation: relates
  target_type: ticket
  target_id: 84
- relation: relates
  target_type: ticket
  target_id: 87
- relation: relates
  target_type: ticket
  target_id: 92
---
## Current project paths

At repository baseline 0117d6f, config/models.yaml declares an official OpenAI-compatible DeepSeek provider but no model row selects it. The configured DeepSeek formatter uses Bailian and current chat/vision use the Gemini relay. DSH has separate search and model credentials. The options below are proposed direct integrations, assessed from documentation. No paid call or account entitlement check was performed in this review.

## Feature-by-feature use

| Capability | Proposed AGLI use and test |
|---|---|
| Fast web and image search | Answer an expressly current public question with citations. With thinking off, let Flash decide whether it needs to search. Carry readable source links through group reply formatting. Display image hits as references. Sticker adoption still uses the owner's media rules. Direct standalone Search currently offers text search only. |
| Deep thinking search | Use the thinking path for multi-source questions and watchlist investigations. It can search, open pages it found and locate passages. Compare with DSH on the same question. Deep-search page openings may provide only metadata to the caller, which is inadequate for a durable passage-level evidence record. |
| Read supplied links | When a user asks about a public URL, enable deepseek_read_url for a purpose-built turn containing only the intended links. Never silently include other people's buffered messages or private/signed URLs. Test unreadable pages and absence of a fabricated summary. |
| Standalone Search and Fetch | For research, issue search queries, choose URLs, fetch the returned Markdown and record URL, retrieval time, response status, returned text and its digest alongside the claim. Distinguish snippets, model excerpts and retrieved Markdown. This builds on #44. A hash attests to the saved provider representation. Empty or failed fetches leave the claim unverified. Keep the DSH path for comparison. |
| Files and parsed document Q&A | Let an owner ask about an explicitly shared PDF or slides. Use text extraction for prose or rich layout when figures matter. XLSX and supported text formats use user_data. Set expiry on upload (1 hour to 30 days), scope file IDs to the originating task/chat and clean them up. Otherwise files can remain indefinitely. Test deletion/expiry, an oversized input and a second-turn question. Each repeated reference contributes context tokens. |
| Standalone OCR | Extract screenshot or scanned-page text with page indices and bounding boxes. Score transcription and layout separately from Gemini's visual interpretation in #56. The endpoint returns page output without a Files object or chat call. A length-limited page is partial. An OCR result alone cannot name a pictured character or establish a meme's meaning. |
| ASR | Compare paid batch transcription in #62 on Mandarin, names, mixed speech and noise. QQ preserves a voice attachment but common SILK/AMR needs conversion to accepted WAV. Telegram needs voice intake first, then validate Ogg Opus. Enforce 8 MiB / 120 seconds at the provider boundary. Test stereo clips (only left is recognized), failed conversion and meaningful error feedback. Keep locally restricted audio and its transcript on an eligible local path. |
| TTS | Compare the two presets on owner-selected finalized replies in #66. HTTP returns WAV/PCM; WebSocket also offers raw Opus packets. Verify actual Telegram voice packaging and QQ record playback separately. Start with optional short read-aloud, preserve a text fallback on definitive failure, and reconcile uncertain sends before retry. Measure persona fit, pronunciation, latency and billed characters. |
| Model safety | For a DeepSeek-backed outward reply, enable the documented safety setting and keep text provisional until the final refusal/finish event. The current OpenAI-compatible stream reader accumulates text without preserving the final content-filter outcome. Test a late refusal, blocked input and image rejection. Avoid sending, storing, voicing or re-routing rejected output as accepted text. AGLI independently checks audience and processor before sending any input. |
| Conversation titles | Optionally title an owner research session or archived thread on its first request. Titles have no separate charge but can delay completion by up to five seconds or be absent. Keep the title in the same disclosure scope and use a local fallback label. Low priority for ordinary group replies. |
| User metadata | latest_reminder can express date, weekday, optional location, App/Web and language immediately before the user turn in Messages or Chat Completions. Responses lacks this extension. Use it only where the fields are truthful and useful. Avoid inferring location or adding IDs. Test relative-date questions including timezone boundaries against AGLI's existing context. |
| Open-source frontend | MIT chat-demo is an interaction reference for citations, document cards, voice, refusal rendering, edit/regenerate/continue, Markdown/Mermaid and import/export. Use synthetic replays to compare AGLI output with the reference. Browser-local credentials/history and its overwrite-on-import behavior are unsuitable defaults for a shared bot. Keep imported history separate from persona memory absent a scoped design. |

## Evaluation order

1. Use public or synthetic materials for a small metered research comparison. Track accuracy of cited passages and unsupported claims, not just attractive citations. Example at the posted CNY rates: one successful standalone search plus three successful fetches costs 0.020 before inference.
2. Exercise fast/deep/link-read turns through a direct adapter separate from Bailian. Preserve native protocol blocks, signed read-link results and citations in ephemeral per-session state. Keep opaque signatures out of durable memories. Replay a second turn and assert it actually uses the page. Current OpenAI-compatible text-only output loses this state. The Messages adapter already retains native blocks but still needs fixtures for DeepSeek events. Separate acquisition from a strict structured-output step. Hosted tool mode has restrictions on response format and some generation controls.
3. Use #56/#62/#66 for matched document, OCR and voice trials, reporting sample size, accuracy, p50/p95, failures and actual units. Example charges excluding inference: 100 successful OCR pages = CNY 0.15, 10 minutes of ASR at the temporary rate = CNY 0.065, and 100 Han characters of TTS = CNY 0.03 at that rate. Record the price date and keep native currency.
4. Before routing a real audience to a candidate path, test allowed and denied processor policy, cross-chat file/signature isolation, bounded deadlines and the #87 freeze case. For hosted web tools reconcile billed tool_search/tool_fetch/tool_image_search, separately from display counters and model tokens. max_tokens caps each inference step. Deep search may issue many tools in one turn. For strict per-call cost control, prefer client-issued standalone operations. If final usage is unavailable after interruption, record cost as unknown pending reconciliation. Uploaded-file parsing and OCR may finish and bill after disconnect.

Recommend default routes only after those comparisons. Publish the benchmark result and any new routing choice as separate records.

## 已选实施范围 · 2026-11-17

主人选定聊天中直接读取用户给的公网网页（#21），以及 PDF、Word、PowerPoint、Excel 和截图（#101）。截图使用当前策略允许的视觉模型优先，表格与复杂版面走官方文件解析。对应的验收从 QQ 或 Telegram 收件开始，包括原会话追问、材料出处、处理方限制、超限与解析失败。#55、#56 中的截图用例复用这批证据；代表集质量、延迟和真实渠道回执分别验收。

主人私聊中的任务可携带材料进入运行根的工作区；研究与编码通过独立场景 work_review 交给 DSF 逐轮审查（#102），最终裁决决定任务状态及知识入库（#92）。仓库配置选用百炼已有的 DSF 别名；官方读取凭据与 DSH、百炼审查凭据分别配置。其他搜索模式、声音及相应成本对比仍归 #100。
