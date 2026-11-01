---
id: "n-agli-dec-intent"
title: "Intentions via reply markers and an owner console"
kind: "decision"
state: "proposed"
relations: [{"type": "relates", "target": "note:n-agli-dec-no-group-speculation"}]
abstract_zh: "聊天模型用回复里的标记表达“记任务/转告主人”，运行时只认主人私聊里的任务标记；斜杠命令是确定性的控制通道，非主人只能用已发布的能力命令。"
abstract_en: "Reply markers carry intentions (honoured only from the owner); slash commands are the deterministic control path, and non-owners may use only published capability commands."
---
No tool-calling API yet: the chat model signals tasks with <任务 类型=...> and relays with <转告主人>, defined in L1 so the cache prefix stays stable. The runtime only honours task markers in owner chats. Owner slash commands (/task, /run, /stop, /freeze, /approve ...) are the deterministic control path.

The same command path serves published capabilities (n-agli-dec-no-group-speculation): non-owners may invoke only commands in that registry, checked by platform ID before dispatch. A task marker produced while answering a non-owner is never honoured, even if the model emits one.