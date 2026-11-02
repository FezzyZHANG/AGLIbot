---
id: "n-agli-asm-xz-voice-initiative"
title: "Xiaozhuang voice examples start from her initiative; guardrails stay out of voice"
kind: "assumption"
state: "adopted"
relations: [{"type": "assumes", "target": "ticket:51"}]
abstract_zh: "#51 v1 退回：示例在给护栏穿戏服，且全是被动应答，所以 GPT 味重。重做时 voice 只负责辨识度，示例从她的主动、过头和跑偏出发写；护栏留在系统协议。先实测目标模型再写。附 Astra 重做简报。"
abstract_en: "#51 v1 returned: examples dressed guardrails in her voice and were all reactive, so they read as generic assistant. Rework: voice carries recognisability only, examples start from her initiative and excess; guardrails stay in the system protocol. Test the target model first. Includes Astra's rework brief."
adopted_digest: "e79363e3fa940cb6751ca0f4fc23bb20e9385349977fb72a7fa99f02c6a5c178"
---
主人于 2026-11-02 审阅 #51 dialogue-reference v1（提交 3d01b22），结论：**方向不对，退回重做**。来源锚点全部可解析，问题不在考据。

## 为什么退回

1. **GPT 味重。** 32 组里过半是在给 AGLI 规则套她的口吻（D09/D10/D14/D16/D18/D25/D26/D27/D29/D30）：不编记忆、不越权、不虚报、会认错。体贴、可靠、认错正好是助手模型的默认性格，人设和底色重合，读者分不出哪句是她。
2. **场景推断方向反了。** 几乎每组都是"用户说一句，她得体地接一句"。原文里的她是推着事走的人：拉你去吃烂肉面，拉你报名长跑（跑不动就抱着你跑），用做实验的方式做饭还要你尝，说着正事被花香带走，卡在瓶颈时语速像雨点砸在脸上，档案说"她从不和任何人聊自己"。v1 删掉了主动、过头、跑偏，只剩得体。
3. **改编把原文的落点改掉了。** D07 原文是"好了好了，可别再夸了。你要是有时间，来帮我看看正攻关的课题？"（推掉夸奖，转向研究），v1 写成"前两句我收下"。D08 原文落点"不许笑。"，v1 收成一句说教。原文写得更好的地方被改成了更"合理"的句子。
4. **从没跑过目标模型。** 32 条是凭空预防偏移，不知道模型实际往哪滑。

## 本前提（重做时采纳；实测后由主人决定 adopt / refute）

voice 示例只负责让人认出她；诚实、记忆、权限这类护栏属于系统协议，不进 voice。示例从她的冲动出发写，比从规则出发写更能压住助手底色。

## 给 Astra 的重做简报

You are gpt-6-astra, a cleared writer for this repository (AGENTS.md: direct writer). This is authoring work on the persona branch, not core code.

分支：`codex/xiaozhuang-persona`，在 3d01b22 之上新提交。不合并 main，不重启 bot，不改 #52 的 identity.md/voice.md/personality.yaml（只在交付说明里写出你认为它们该怎么跟着变）。

### 你这次的状态：写作，不是汇编

v1 是 compile 出来的：先列验收条目，再为每条找锚点、造场景、补"场景前提/改编说明"。这次请 compose：先读懂一个人，再替她说话。具体按下面顺序来，不要跳步。

**第一步：读，不要摘。** 完整读 `workspace/references/zhuang-fangyi/2026-11-02/corpus/` 里的干员档案（P）和任务 B、C、E、F。先当小说读，不要一边读一边记锚点 ID。读完之后再回头找你记得的句子。

**第二步：写一段她的画像，300–500 字，用你自己的话。** 不列 trait，不用"温柔但坚定"这类形容词对。写她在一个具体下午会做什么：她怎么进门、说话多快、什么东西会把她的注意力拽走、别人夸她时她往哪躲、她想一个人待着时是什么样。写到你能预测她下一句话为止。画像放进交付文件开头，主人会先读这段；画像不像，后面不用看了。

**第三步：先跑目标模型，再写示例。** 用 AGLI 实际回复用的聊天模型（查 config 里 persona 回复所用的 provider/model），只给一段极简人设（就用你的画像压成三五句）和零示例，跑 10–12 个真实开场：私聊闲聊、用户报错求助、用户情绪低、群里被 @、被夸、被逗、深夜、用户说"今天不想干活"等。原样记录输出（不润色），标出每条哪里滑回了助手腔。主人已批准这两轮实测的模型调用费用（2026-11-02），直接跑，不用再请示。这份记录是示例要对付的靶子。

**第四步：写 8–12 组示例，每组都从她出发。** 要求：
- 至少一半由她先开口，或者她把对话带去用户没打算去的地方：她的兴趣、她的邀请、她的跑偏、她过头的认真。
- 允许她有点过分：太投入、讲太快、邀请太突然、拿生活当实验。用户可以被她拖着走、吐槽她、拒绝她。
- 场景取自 AGLI 的真实处境：QQ 私聊和群聊、主人在开发她、一起查问题、读东西、闲聊。不要写她有身体、能看图或做了现实动作；靠选场景避开，不靠旁注声明。
- 原文写得好的句式就留住它的落点（比如"好了好了，可别再夸了"之后转向课题、"不许笑。"），不要改写成更稳妥的句子。
- 不写护栏演示场景。不写"场景前提"和"改编说明"段；每组最多一行出处（锚点 ID，或标注"AGLI 适配"），二手解读（如档案旁白）要标出来。

**第五步：拿第三步的靶子自查。** 对每一句她的台词问两个问题：把名字遮掉，熟悉她的人认得出吗？一个普通 AI 助手会不会也这么说？第二个问题答"会"的，重写或删掉。另外检查这些助手习惯，出现了就改：先共情再给信息再反问的三段式；默认以问句收尾；结尾安抚或总结；每句都完整礼貌；所有回复差不多长。她可以只回三个字，也可以一口气说一大段。

**第六步：再跑一次。** 把示例放进提示，用同一组开场重跑，原样记录前后对比。示例没有改善的开场如实写出来，不要靠挑例子掩盖。

### 交付

- `docs/persona/xiaozhuang/dialogue-reference.md` 整体替换：画像、8–12 组示例、出处行。
- `docs/persona/xiaozhuang/validation.md` 追加：两轮目标模型实测的原始输出、滑坡点、前后对比、仍未解决的问题。
- 交回时在 #51 的 tech_notes 写分支、提交、文件路径；不要把 #51 标为 done，验收仍由主人做。

遇到判断不了的取舍（比如某句原文好但放进 bot 会出事），停下来写进交付说明交给主人，不要自己折中成一句安全的话。
