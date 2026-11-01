---
id: "n-agli-dec-no-cloud-split"
title: "No cloud/daemon split for now"
kind: "decision"
state: "accepted"
relations: [{"type": "relates", "target": "note:n-src-deepseek-superagent"}]
abstract_zh: "暂不采用云端无状态 + 端侧 daemon：AGLI 是桌面 agent，拆分只会增加攻击面。迁到常驻服务器时再议。"
abstract_en: "Not adopting the cloud loop + device daemon split while AGLI is a single-machine desktop agent."
---
Rejected for now: essay section 5 (stateless cloud agent loop, state in a database, a lightweight daemon on user machines, BYOM). AGLI is a desktop agent whose persona, secrets and execution environment already live on one machine; splitting them adds the attack surface the essay itself flags in section 8.4. Revisit if AGLI moves to an always-on server.