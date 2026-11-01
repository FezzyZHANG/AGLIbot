---
id: "n-agli-dec-privacy-files"
title: "Personal runtime content stays out of git"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "ticket:8"}]
abstract_zh: "运行时状态、任务目录、看板和日记不进 git，避免私聊内容进入仓库；mind/state 只保留种子。"
abstract_en: "Runtime state, tasks, board and journal are gitignored so private content never reaches the repository."
---
Runtime state moved from mind/state to data/state (mind/state keeps seeds). work/tasks, work/board.yaml and mind/life/journal are gitignored. This trades the 'git-versioned consciousness' idea for not leaking private conversations into the repository.