---
id: "n-src-deepseek-app-api-20261113"
title: "DeepSeek App APIs released November 13 and official price sheet"
kind: "source"
state: "recorded"
relations: [{"type": "relates", "target": "ticket:100"}]
abstract_zh: "11 月 13 日官方开放托管能力，另开源 MIT 前端示例。记录对话扩展、独立 Search/Fetch/OCR/ASR/TTS、文件有效期及 11 月 17 日人民币挂牌价。"
abstract_en: "Primary-source index, documented protocol coverage, billing units and file lifecycle as checked on 2026-11-17."
---
DeepSeek's November 13 announcement opens hosted App capabilities to developers and separately publishes the chat-demo frontend. The reference model for hosted chatbot extensions is deepseek-flash on the direct API. Messages, Responses and Chat Completions have different event formats and feature exceptions. Standalone Search, Fetch, OCR, ASR and TTS have their own interfaces. Search/Fetch were already available before this release.

## Price snapshot — CNY, November 17

| Meter | Published charge |
|---|---|
| Search or Fetch | 5 per 1,000 billable searches or fetches, including hosted tools |
| Document parsing or standalone OCR | 1.5 per 1,000 successful pages |
| ASR | 0.78 per hour list, temporarily 0.39. Audio duration is billed to the second, minimum one second |
| TTS | 3 per 10,000 billing characters list, temporarily 1.5. Each Han character counts twice |
| Direct Flash, per million tokens | Peak: 0.04 cached input, 2 uncached input, 8 output. Off-peak: 0.02, 1, 4 respectively |

Peak periods are 09:00–12:00 and 14:00–18:00 Beijing time on working weekdays excluding Chinese public holidays. Consult the price page when running a trial. Standalone Search/Fetch charge for a successful response with a result/body. For hosted tools use the documented billed counters. Inference, safety review and previously completed tools can incur costs even on a refused turn. Document references can add model input charges on later turns. The voice discount has no end date established in this note.

## Sources checked

- [Release, November 13](https://api-docs.deepseek.com/zh-cn/news/news261113/)
- [Chatbot feature map and protocol exceptions](https://api-docs.deepseek.com/zh-cn/guides/build_a_chatbot/)
- [Hosted search, citations, replay and billing counters](https://api-docs.deepseek.com/zh-cn/guides/web_tools/)
- [Standalone Search and Fetch](https://api-docs.deepseek.com/zh-cn/guides/search/)
- [Document Q&A and text-only formats](https://api-docs.deepseek.com/zh-cn/guides/document_parsing/)
- [File expiry and deletion](https://api-docs.deepseek.com/zh-cn/guides/files_api/)
- [Standalone page OCR](https://api-docs.deepseek.com/zh-cn/guides/ocr/)
- [Audio recognition](https://api-docs.deepseek.com/zh-cn/guides/asr/)
- [Audio generation](https://api-docs.deepseek.com/zh-cn/guides/tts/)
- [Safety and stream outcomes](https://api-docs.deepseek.com/zh-cn/guides/safety/)
- [First-turn titles](https://api-docs.deepseek.com/zh-cn/guides/conversation_title/)
- [User metadata and protocol support](https://api-docs.deepseek.com/zh-cn/guides/latest_reminder/)
- [Prices in CNY](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)
- [Official reference client](https://github.com/deepseek-ai/chat-demo)
- [Reference client license](https://github.com/deepseek-ai/chat-demo/blob/main/LICENSE)
