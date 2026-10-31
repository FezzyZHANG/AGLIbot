# AGLIbot

AGLI（Artificial Guy-Like Intelligence）：一个像受过良好教育的普通人那样生活的桌面 agent。设计见 [docs/DESIGN.md](docs/DESIGN.md)。

目前处于阶段 1（骨架）：在 Telegram 上和主人私聊，四层缓存友好的 prompt，QCL 表路由，JSONL 存储。

## 运行

1. 用 [@BotFather](https://t.me/BotFather) 创建一个 bot，拿到 token。
2. `cp .env.example .env`，填 `TELEGRAM_BOT_TOKEN`，再至少填一个模型提供商的密钥。
3. 在 `config/acl.yaml` 里填你的 Telegram 数字 ID。不知道的话可以先留 0 启动，给 bot 发条消息，日志会打印出你的 ID。
4. 启动：

```bash
cargo run
```

## 你会经常改的文件

- `config/models.yaml`：QCL 表，平时只需要调 `q` 这一列
- `mind/self/*`：人设草稿，改完重启生效（它属于 L1 宪法层）
- `mind/social/people/owner.md`：你希望他知道的关于你的事

运行数据写在 `data/`（已被 gitignore）：`messages.jsonl` 存聊天记录，`model_calls.jsonl` 存每次调用的 token、缓存命中和花费。
