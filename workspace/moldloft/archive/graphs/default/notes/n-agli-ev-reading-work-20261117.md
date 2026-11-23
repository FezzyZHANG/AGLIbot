---
id: n-agli-ev-reading-work-20261117
graph_id: default
version: 1
title: 网页、附件读取与多轮工作验收（2026-11-17）
kind: evidence
state: recorded
in_graph: true
summary: '记录直接读取网页、PDF 与 Office、原生看图、会话追问及 DSF 多轮审查的验证范围与复现入口。


  Read receipts and reproducible tests for web, document and image intake, chat follow-ups,
  runtime work and DSF review.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links:
- relation: relates
  target_type: note
  target_id: n-agli-idea-deepseek-app-integration
- relation: relates
  target_type: ticket
  target_id: 100
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
  target_id: 93
- relation: supports
  target_type: ticket
  target_id: 101
- relation: supports
  target_type: ticket
  target_id: 102
- relation: supports
  target_type: ticket
  target_id: 21
- relation: supports
  target_type: ticket
  target_id: 92
---
## 工单范围

用户在 QQ 或 Telegram 给出公网网址，读取后能在同一会话追问，对应 #21。用户向聊天发送 PDF、Word、PowerPoint、Excel 或截图并提问，对应 #101。截图路径复用 #55 的原生看图和 #56 的降级验收。#44 已验收的是 DSH 的来源地址匹配。直接 Fetch 返回的正文、OCR 返回的页文以及模型提炼的文件或图像摘录应分别注明出处。#100 另有主动搜索、语音等能力与价格/质量对比，继续独立跟踪。

主人主动将材料交给研究或编码任务后，进入 #102。普通对话不需要先建任务。运行根由 AGLI_ROOT 指定，默认工作目录为其下 work，每项任务分开保存输入与产物。work_review 精确指定仓库已有百炼 DSF 别名 bailian-deepseek-flash。官方读取使用 DEEPSEEK_API_KEY；审查使用百炼凭据。#92 负责最终裁决和知识入口，#93 跟踪运行中及收尾时到达的编码补充。

## 官方服务和模型回执

- 用公开 DeepSeek 接口文档测试了官方 Fetch。在隔离运行的 AGLIbot 里，模拟 OneBot 主人第一轮发网址，回答只确认读到。第二轮没有网址，聊天模型请求中仍有来源正文，得以答出 8192。随后发送 /task 用刚才的网页整理接口限制，运行目录内出现一份带来源、原会话和内容摘要值的任务输入。此试验的聊天回答由本机模拟服务产生。复现记录在 target/deepseek-app-20261117/chat-e2e-1794901178755483600/receipts.jsonl。
- 付费官方测试使用合成 PDF、DOCX、PPTX、XLSX，逐份检查预设校验词。富布局试验使用只有像素的柱状图 PDF，模型答出红色最高、蓝色最低。官方 Files 回执确认删除，上传时另指定一小时到期。OCR 直接传单份文件，可把页号及截断信息带回聊天。旧式 Office 扩展名已实现格式识别，付费样本覆盖的是 OOXML 格式。
- Telegram 附件试验使用本机 getUpdates、getFile 和文件下载服务，模拟私聊发来一份 PDF 与一个以文档形式发送的 PNG。正常消息处理把 PDF 送往真实官方 OCR，同一次聊天推理请求中出现 PDF 校验词、文件名和 PNG 解码的原生 JPEG 图像。聊天端是本机模拟接口。原始记录在 target/deepseek-app-20261117/tg-attachment-fixture/receipt.log。另有常规回归核对另一个聊天收不到材料、任务视觉摘录优先使用允许的原生模型、带凭据网址与转发内容不触发外发读取。
- 真实 work_review 请求只发送合成交付要求与实际 result.txt 摘录。百炼返回 deepseek-v4.1-flash，裁决通过，场景与模型别名在 model_calls.jsonl 中可查。该次延迟 4677 ms，程序的成本估值为 USD 0.000423。记录在 target/work-review-smoke/1794901219321-43324/data/。官方读取的总账以提供商账单为准。

## 工作与数据边界

- 隔离运行的二进制经过 /task、/run、执行者、审查和看板写回。第一例修改后通过，第二例修改后暂缓。两次返工的执行者都收到具体意见；审查服务收到修改后的产物。审查服务与 DSH 在这项可重复试验中使用本机模拟服务。配置省略 work_dir 且启动目录与运行根不同，两项任务均写入运行根的 work/tasks。回执在 target/deepseek-app-20261117/e2e-1794901034234692200/receipts.jsonl。
- 每次运行最多三轮。未解决的修改、暂缓和转交主人保留原因；未覆盖的补充保持可处理。知识整理入口只处理有通过裁决的完成态研究。历史已生成的知识卡片不在本次迁移范围。
- #92 在正常完成处理器里覆盖研究和编码的四种裁决，并对两类任务各加一例审查后才收到的补充。重新读取磁盘上的看板和审查记录，分别得到 Done、Waiting 或重新进入 Inbox。再次 /run 并通过后，先前已解决的自动修改、暂缓记录保留裁决和后继审查号，主人待决定的事项独立保留。
- #93 另做了隔离二进制试验。主人发 /code 和 /run，第一轮模拟执行者暂停，此时从模拟 OneBot 正常私聊入口补充一项要求。看板的 followups.md 在执行者恢复前已落盘。首轮审查虽通过，第二轮执行者仍收到 ADD_ORCHARD_FIELD 并交回新版产物，复审后任务才 Done。此例使用本机模拟模型与编码执行者。记录在 target/deepseek-app-20261117/code-followup-1794901742502593600/receipts.jsonl。
- 按平台及会话隔离材料，冻结状态和处理策略在直接读取前检查。任务资料只接受同一主人私聊的交接。网页正文、OCR 和模型描述均可能有提取偏差，其哈希用于核对保存的表示。每轮阅读材料上限三项，单份文档最多 12 MiB，摘录最多约 12,000 字符。

## 可重复验证

本轮 #21、#92 验收后，Work & knowledge 组的 #17、#18、#19、#20、#21、#22、#44、#92 八张均为 Done。沿用现有命名，整组移至 Archive / Work & knowledge，保留编号、状态和依赖。Integrations / DeepSeek 的 #100、Multimodal / Vision 的 #55/#56、Orchestration 的 #30/#33 均仍有独立验收项。

最终冻结的工作树执行 cargo test --locked --offline，386 项通过，0 项失败，19 项按标记忽略；cargo build --locked --offline 与 git diff --check 通过。可单独执行 live_reader_smoke 和 live_dsf_work_review_receipt 两个付费回归。前者需设置 AGLI_READER_LIVE=1 及官方密钥；后者使用已配置的百炼密钥。严格 Clippy 的七处既有诊断与 11 月 15 日的基线一致。运行环境为源码构建及隔离测试实例，线上 QQ、Telegram 服务未部署此次改动。#56 的标注代表集与延迟分布、#87 的独立超级开关及 #100 的其余评估继续跟踪。
