---
id: n-agli-q-napcat-webui
graph_id: default
version: 1
title: Should the NapCat WebUI listen on all interfaces?
kind: question
state: resolved
in_graph: true
summary: '不。WebUI 改为只监听本机（127.0.0.1:6099），和 AGLI 用的 OneBot WS 一致。


  No. The WebUI now binds to 127.0.0.1:6099, matching the localhost-only OneBot socket.'
created_at: '2026-11-23T05:36:44Z'
updated_at: '2026-11-23T05:36:44Z'
adopted_body_hash: null
links: []
---
NapCat's WebUI listened on `::` port 6099 (all interfaces, IPv4 and IPv6), reachable from the LAN with a short token. The OneBot WebSocket added for AGLI listens on 127.0.0.1:3001 with its own token.

Resolved 2026-11-01: C:\dev\NapCat.Shell\config\webui.json `host` set to 127.0.0.1 (backup webui.json.bak-20261101). Takes effect on the next NapCat restart. For remote administration, use an SSH tunnel rather than reopening the bind.