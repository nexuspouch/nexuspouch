# Agent 会话历史管理设计（Session History）

> 状态：P1–P3 骨架已落地（2026-08-02）——`sessions` 空间 + strict 清洗 +
> `--bind`；MCP `session_recall`；网关 transcript 旁路（debounce 写 jsonl）。
> 格式适配器 / 消息级分块仍可增强。
> 定位：把各 agent 的会话历史纳入 Nexuspouch 管理——跨设备备份、版本化、
> 全文与语义召回。会话历史是记忆层的最大数据源，与
> [VECTOR_SEARCH_DESIGN.md](VECTOR_SEARCH_DESIGN.md) 直接衔接。

## 0. 结论摘要

- 新增 `sessions` 空间（默认 private），统一管理各 agent 会话；
- P1：文件级摄取（复用 M6 绑定机制）+ 格式适配层 → 跨设备备份 + FTS5 全文；
- P2：语义召回（分块 embedding，接向量搜索）→ `session_recall`；
- P3：agent-bridge 网关实时旁路捕获 → 覆盖非文件型 agent；
- 隐私红线：默认 private + 索引前敏感信息清洗，清洗级别用户可选。

## 1. 现状与动机

| Agent | 位置 | 格式 |
|-------|------|------|
| Claude Code | `~/.claude/projects/<项目>/<session>.jsonl` | JSONL 转写 |
| Codex | `~/.codex/sessions/<session>.jsonl` | JSONL 转写 |
| Cursor | `.cursor/` 会话目录 | 各有差异 |
| ACP agent（经 agent-bridge） | 网关侧 transcripts | ACP 结构化事件 |

- 会话历史是 agent 最贵重资产（上下文/决策/偏好），现散落隐藏 dotfile、无备份、换机即丢；
- 它是记忆层原材料：语义召回（"上次怎么解决 X 的"）需要历史语料；
- 跨设备连续性：PC 会话 → 手机 agent 可引用召回，坐实"agent 协作空间"定位；
- 竞品空白：Mem0 只存提取摘要；云工具锁在各自生态；无人做跨 agent 本地历史管理。

## 2. `sessions` 空间 profile

```json
{
  "name": "sessions",
  "visibility": "private",
  "encryption": "client",        // 可选；默认 none + 索引前清洗
  "retention": "keep_last",      // 会话版本按策略修剪
  "import_grant": "allowed"      // 换机可迁移
}
```

- 布局：`<device_id>/sessions/<agent>/<session_id>.jsonl`（规范化 NDJSON）；
- 原始文件与索引分离：`sessions/` 存规范化转写，`<task>/.nexuspouch/manifest.json`
  存会话摘要与血缘；
- 走现有空间属性模型：ACL / 授权 / agent 配额自动适用。

## 3. 统一会话 schema（NDJSON 每行一条）

```json
{
  "agent": "claude-code",
  "session_id": "abc123",
  "project": "nexuspouch",
  "seq": 1,
  "ts_ms": 1721300000000,
  "role": "user | assistant | tool",
  "content": "…",
  "tool_call": {"name": "store_write", "args": "…"},
  "model": "claude-sonnet-4",
  "cost": {"input_tokens": 100, "output_tokens": 200}
}
```

适配器（可插拔，格式漂移只改 adapter）：

| adapter | 输入 | 说明 |
|---------|------|------|
| claude-jsonl | `~/.claude/projects/**/*.jsonl` | 消息/工具调用/结果映射 |
| codex-jsonl | `~/.codex/sessions/*.jsonl` | 同上 |
| acp-transcript | agent-bridge 网关事件 | 结构化，最干净 |
| generic-jsonl | 其他 | 最佳努力映射 |

## 4. 敏感信息清洗（隐私红线）

- sessions 空间默认 private；**索引前必须清洗**；
- 清洗级别（用户可选，存于空间 profile 或绑定配置）：
  - `strict`（默认）：token/密钥/路径/邮箱启发式脱敏后索引，原始文件仍完整保留在
    private 空间（不索引原始内容）；
  - `full`：完整索引（用户显式开启，用于本地单用户场景）；
- 清洗在索引层执行，不改写原始转写文件。

## 5. 里程碑

### P1 文件级摄取（≈1 周）

- 复用 M6 绑定机制：把 `~/.claude/projects/`、`~/.codex/sessions/` 作为绑定目录，
  摄进 `sessions/<agent>/`；
- 格式适配层 + 统一 NDJSON 规范化；
- 会话追加 → commit（5s debounce 批量）→ `versions.list` = 会话时间线；
- FTS5 全文索引（消息正文）+ 跨设备备份（镜像到 master 天然完成）；
- 命令：`nexuspouch --sessions claude-code=~/.claude/projects [codex=~/.codex/sessions]`
  或复用 `--bind` + sessions 空间约定；
- 验收：两个 agent 的历史文件被摄取、可 `store_search(space=sessions)` 命中、
  `versions` 有版本、手机/另一设备可见。

### P2 语义召回（并入向量搜索）

- 会话按消息/1000-token 分块 embedding（VECTOR_SEARCH_DESIGN §3.3/§3.4）；
- `store_search(q, space=sessions, semantic=true)` 返回
  `{session_id, message, snippet, uri}`；
- 每会话 manifest.summary（"这个会话解决了什么"）进索引 title；
- MCP 工具 `session_recall(q)`；
- 验收：跨会话语义召回命中（如"上次部署报错怎么解决的"）。

### P3 网关实时捕获 — 已落地（骨架）

- agent-bridge `SessionTranscriptSink`：prompt 用户轮 + drain 助手轮 →
  debounce 5s → `StoreToolsClient.write` 到 `sessions/<engine>/<session>.jsonl`；
- 启用：`NEXUSPOUCH_URL` + `NEXUSPOUCH_DEVICE` + `NEXUSPOUCH_ADMIN_TOKEN`
  （有 `NEXUSPOUCH_ROOT` 时 URL 默认同机 `:8787`）；`NEXUSPOUCH_TRANSCRIPT=off` 关闭；
- 历史会话回填仍走 P1 文件绑定；
- 验收：网关对话后 `store_search`/`session_recall` 可命中转写。

## 6. 关键决策点（带推荐）

| # | 决策 | 推荐 | 备选 |
|---|------|------|------|
| D1 | 空间形态 | 新增 `sessions` profile（private 默认） | 复用 files + 目录约定 |
| D2 | 清洗默认级别 | `strict`（脱敏后索引） | full 显式开启 |
| D3 | 版本策略 | 5s debounce 批量 commit + keep_last | 每条消息一版（版本爆炸） |
| D4 | 摄取入口 | 先 `--bind`/绑定复用，P3 再网关旁路 | 直接先做网关 |
| D5 | 原始文件保留 | 完整保留于 private 空间 | 只存摘要（不建议） |

## 7. 风险与对策

| 风险 | 对策 |
|------|------|
| 格式漂移（agent 更新格式） | 适配器可插拔 + fixture 用例 |
| 数据量大（完整转写） | 索引只进清洗后文本；原始文件按保留策略修剪 |
| 隐私泄露 | private 默认 + strict 清洗 + 用户显式升级 full |
| 高频写入 | debounce 批量 commit + 事件合并 |

## 8. 与现有能力的关系

- 摄取走 M6 绑定 / loopback 写路径 → 版本、血缘、索引、交接钩子全触发；
- 语义召回走 H 向量搜索（P2 依赖其 P1）；
- `sessions` 空间走 Step 2 空间属性模型，ACL/配额自动适用；
- 与 ShePaw `external_memories` / `memory.digest.offer` 概念互补：
  会话历史是原始语料，蒸馏摘要进记忆交换。
