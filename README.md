# AGLIbot

AGLI（Artificial Guy-Like Intelligence）：一个像受过良好教育的普通人那样生活的桌面 agent。设计见 [docs/DESIGN.md](docs/DESIGN.md)。

目前处于阶段 1（骨架）：在 Telegram 上和主人私聊，四层缓存友好的 prompt，QCL 表路由，JSONL 存储。

## 运行

1. 用 [@BotFather](https://t.me/BotFather) 创建一个 bot，拿到 token。
2. `cp .env.example .env`，填 `TELEGRAM_BOT_TOKEN`，再至少填一个模型提供商的密钥。
3. 在 `.env` 里填 `OWNER_TELEGRAM_ID`（你的 Telegram 数字 ID）。不知道的话可以先留空启动，给 bot 发条消息，日志会打印出你的 ID。
4. 启动：

```bash
cargo run
```

百炼 Token Plan 使用 `BAILIAN_API_KEY` 和 `BAILIAN_BASE_URL_OAI`，支持 `.env` 或进程环境变量；
环境变量优先，地址未设置时使用北京 Token Plan 地址。Windows 新增用户环境变量后，需要重启终端或启动 bot 的应用。
这里走 OpenAI 兼容接口，`BAILIAN_BASE_URL_ANT` 不需要配置。
默认加入 Qwen Flash / Plus / Max 和百炼 DeepSeek Flash / Pro，具体模型 ID 在 `config/models.yaml` 中修改；
只有百炼密钥也能覆盖所有场景。Qwen 开启思考与流式请求，回复收齐后发送，推理内容不发到 Telegram。
新增模型可配置 `stream` 和 `enable_thinking`；原有 Anthropic / DeepSeek 直连接入保持兼容。
支持模型与思考参数参见[百炼官方接入文档](https://help.aliyun.com/en/model-studio/qwen-code)。
Token Plan 条目的 `c` 是路由估值，日志中的美元花费也是估算，不代表套餐实际扣费或剩余额度。

Telegram 的收发消息走环境变量 `TELEGRAM_PROXY` 指定的代理（例如 `http://127.0.0.1:7890`），
需要先启动本地代理；留空则恢复 HTTP 客户端默认代理行为。此配置只作用于 Telegram；模型客户端继续遵循系统代理配置。

主人 ID、代理这类因人因机器而异的值都放在 `.env`（已被 gitignore），`config/` 里只写变量名。

## 你会经常改的文件

- `config/models.yaml`：QCL 表，平时只需要调 `q` 这一列
- `mind/self/*`：人设草稿，改完重启生效（它属于 L1 宪法层）
- `mind/social/people/owner.md`：你希望他知道的关于你的事

运行数据写在 `data/`（已被 gitignore）：`messages.jsonl` 存聊天记录，`model_calls.jsonl` 存每次调用的 token、缓存命中和花费。
