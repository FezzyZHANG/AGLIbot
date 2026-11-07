# 部署

在线的 bot 不从开发 checkout 运行。这里说明两个目录分别管什么，以及“当前跑的是哪个版本”记在哪里。

## 两个目录

| 目录 | 作用 | 例子 |
|---|---|---|
| 源码仓库（`AGLI_SOURCE`） | git、Moldloft、开发；进化闸门在这里建 worktree 和提交 | `C:\dev\AGLIbot` |
| 运行时根（`AGLI_ROOT`） | 在线 bot 的 `config/`、`mind/`、`data/`、`work/`、`.env` | `C:\dev\AGLIbot-runtime\<名字>` |

运行时根里有两个本机文件，都不进版本库：

- `runtime.json`：`{source}`，供脚本使用。
- `.env`：从源码仓库的 `.env` 复制而来，`MONGO_DATABASE` 指向这个运行时自己的库。bot 先读 `AGLI_ROOT/.env`，再读工作目录下的 `.env`；先读到的值优先。

没有设置 `AGLI_SOURCE` 时，源码仓库就等于 `AGLI_ROOT`，也就是旧的单目录用法，测试和本地开发不受影响。

## release 指针

`<运行时根>/data/release.json` 是“应该运行哪个版本”的唯一记录：

```json
{ "commit": "…", "exe": "…\\aglibot.exe", "exe_sha256": "…", "origin": "manual | evolution:<id>", "at": "…" }
```

有两条路线会写它，启动只认它：

- **手动部署**：运行 `ops/deploy.ps1 -Runtime <根> [-Commit main] [-Force]`。
  - 脚本在临时 worktree（`%TEMP%\aglibot-deploy`，用完即删）中检出目标提交，用 `cargo build --locked --release` 构建。target 目录是源码仓库的 `target/deploy/`，各次部署共用，只重编改动过的部分。
  - 只把 `aglibot.exe` 复制到 `<根>/data/releases/<commit>/aglibot.exe`。
  - 列出这个提交的 `config/` 与运行时 `config/` 的差异，只提示，不同步。
  - 写入指针（`origin: manual`），然后重启。
- **进化**：闸门把 release 构建到 `<根>/data/evolution/releases/<id>/`。新进程通过校验并写入 `running` 回执后，写入指针（`origin: evolution:<id>`）。

**防止回滚**：如果目标提交不包含当前指针记录的提交，`deploy.ps1` 会拒绝执行。典型情况是进化跑在未合并的 `codex/evolution-<id>` 分支上。先把它合并进要部署的分支，或者确认要撤掉它后加 `-Force`。

`ops/start-bot.ps1 -Runtime <根> [-Restart]`：
- 先校验指针里 exe 的 sha256，有别的 aglibot 在运行时拒绝启动。
- 通过 WMI 启动 bot，使它不挂在调用者的进程树下。
- 把 pid 写入 `<根>/data/logs/current.json`，日志写在 `<根>/data/logs/bot-<时间>.{out,err}.log`。
- 弹出的 cmd 窗口实时显示日志：bot 把日志同时写到控制台和 `AGLI_LOG_FILE`（即 `.out.log`）。stderr（panic、启动错误）只写 `.err.log`，bot 退出后才显示在窗口里，窗口停在 `pause` 等待按键。关闭还在运行的窗口会连 bot 一起结束。
- 加 `-Restart` 时，只停止 `current.json` 记录的那个进程，并关掉它的窗口。

`AGLI_LOG_FILE` 需要 bot 本身支持。用 `start-bot.ps1` 启动不支持它的旧 release 时，日志只出现在窗口里，不会写入 `.out.log`。进化重启的 bot 没有窗口，日志照旧写在 `runtime.log`。

bot 启动时会在日志里写一行 `release <commit> (<origin>)`。如果正在运行的 exe 与指针不一致，同一行会带上 WARNING。

## 表情包页面

`ops/start-stickers-web.ps1 -Runtime <根> [-Port 8790] [-Restart]` 用指针里的 exe 启动 `aglibot stickers-web`，监听所有网卡，只接受本机和局域网（私有地址）的连接，没有登录：同一局域网里的任何人都能查看、修改和删除。局域网访问还需要放行防火墙的这个端口。pid 记在 `<根>/data/logs/stickers-web.json`。它是单独的进程，没有窗口，bot 重启时页面照常可用；`deploy.ps1` 重启 bot 后会一起重启它，机器重启后要手动再跑一次这个脚本。

向量依赖本机的 Ollama：要在运行，并已 `ollama pull bge-m3`，否则向量排队等待，聊天不受影响。

## config 的两份

源码仓库的 `config/` 是默认值和评审对象。运行时的 `config/` 是在线实际生效的一份，可以有人设相关的差异，比如别名、固定模型。改了仓库里的 config 并不会影响在线 bot；要上线，就在运行时那份里改。每次部署打印的差异列表就是用来发现两份是否漂移的。

L4 的文件系统权限隔离（DESIGN 第 6 节）尚未实现：运行时根目前只是另一个目录，还没有 ACL 限制。
