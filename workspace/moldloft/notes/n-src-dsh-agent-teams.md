---
id: "n-src-dsh-agent-teams"
title: "dsh-agent-teams v0.1.28: attempts, handoff and mailbox source"
kind: "source"
state: "recorded"
relations: []
abstract_zh: "NanmiCoder/dsh-agent-teams 插件 v0.1.28（3331b90）的源码出处：执行批次、停止接管、冷恢复和持久化邮箱。用于幂等性工单参考，未在本机运行。"
abstract_en: "Pinned source for dsh-agent-teams v0.1.28: execution attempts, handoff, cold recovery and mailbox receipts; statically inspected for AGLI idempotency work."
---
Repository: https://github.com/NanmiCoder/dsh-agent-teams
Reviewed revision: 3331b903f0e954d59ac69be1a75fd4fd7e1de20e (2026-10-30), package version 0.1.28.
Inspected on 2026-11-01 through the repository API; static source review, not a local runtime experiment. Re-checked later on 2026-11-01: main was still at this revision.

Relevant implementation:
- Attempt generation and revocation: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/state.ts#L171
- Reassignment revokes the old attempt, stops the previous activation, then permits dispatch: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/tools.ts#L1689
- Member updates validate the current attempt; terminal results are retained: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/tools.ts#L1955
- Scheduler distinguishes parked live attempts from cold recovery, and serializes task claims: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/scheduler.ts#L353
- Mailbox delivery leases and separate delivered/read timestamps: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/state.ts#L514
- Delivery acceptance, retry and discard of obsolete messages: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/scheduler.ts#L324
- Single-process mutation queue: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/src/state.ts#L58
- Product scope and limitations: https://github.com/NanmiCoder/dsh-agent-teams/blob/3331b903f0e954d59ac69be1a75fd4fd7e1de20e/README_ZH.md

This is a DSH multi-member task-coordination plugin. Its higher concurrency, automatic dispatch and member handoff create a different workload from AGLI's current single active delegated job. The source demonstrates mechanisms, not measured incident frequency or net benefit for AGLI. It does not prove exactly-once external effects across process crashes.