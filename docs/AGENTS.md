# Agent 接入指南（Nexuspouch）

Nexuspouch 定位为异构 AI agent 之间**本地优先的产物交接与记忆总线**：任何 agent
拿到一个 `store://` URI，就能读写、校验、追溯产物，且数据默认不出用户自己的硬件。

> 里程碑状态：M0-M3 + M4（agent 身份、作用域与配额）已落地；M5 检索。
> 能力按里程碑逐步接通，
> 本文档的「目标形态」一节描述的是已实现能力与后续增量。

## 1. 当前可用（M0 起）

无头节点自带可编程 HTTP API（Bearer token 或 loopback），任何语言一行 curl 即可接入：

```bash
TOKEN=<scoped-token>   # 或 NEXUSPOUCH_ADMIN_TOKEN
BASE=http://127.0.0.1:8787

# 写入（store 帧：write.begin → write.chunk → commit）
curl -s -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"op":"write.begin","payload":{"space":"artifacts","path":"task-41/out.txt","size":5,"sha256":"<sha256>"}}' \
  "$BASE/api/v1/store"

# 读取
curl -s -G -H "Authorization: Bearer $TOKEN" --data-urlencode \
  "uri=store://artifacts/aaaaaaaaaaaaaaaa/task-41/out.txt" "$BASE/api/v1/read"

# 列目录 / 元数据 / 事件
curl -s -G -H "Authorization: Bearer $TOKEN" --data-urlencode "uri=store://files/aaaaaaaaaaaaaaaa/" "$BASE/api/v1/list"
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/events/recent"
```

完整端点见 [docs/API.md](API.md)，线协议见 [docs/storage_protocol_spec.md](storage_protocol_spec.md)。

## 2. MCP 服务器（M1 起可用）

### 2.1 MCP 服务器（`nexuspouch mcp`）

任何支持 MCP 的 agent（Claude Code、Codex、Cursor、本地框架）通过配置即接入。
需要节点守护进程先运行（`nexuspouch mcp` 复用本机 `/api/v1`，不另开端口）：

```json
{
  "mcpServers": {
    "nexuspouch": {
      "command": "nexuspouch",
      "args": ["mcp", "--root", "/var/lib/nexuspouch", "--token", "<scoped-token>"]
    }
  }
}
```

暴露的工具（与 HTTP API 一一对应，均可直接调用）：

| 工具 | 作用 |
|------|------|
| `store_write` | 写文件，返回 `store://` URI |
| `store_read` / `store_read_chunk` | 读文件 / 分块读 |
| `store_meta` / `store_list` | 元数据 / 列目录 |
| `store_search` | 检索（M5 接通） |
| `store_watch` | 事件订阅（M3 接通） |
| `store_space` | 各块空间占用（空间治理） |

### 2.2 交接与事件（M3 起可用）

- 状态机：`committed → published → acked`，覆盖旧版本 → `superseded`（可读不可改）；
  artifacts 空间默认 published，files 空间需 `publish: true`；
- `handoff.create`：commit + 发布 + `context`（任务描述/验收标准），事件
  `handoff.created`；`handoff.ack {uri, agent_id}`：消费确认，幂等，事件
  `handoff.acked`；`artifact.state`：状态 + 血缘（producer / parent_uris / context）；
- 事件带单调 `seq` 并持久化到 `.system/events.jsonl`；掉线后用
  `/api/v1/events?since=<seq>` 重放历史再切实时，不丢不重；
- 以上 op 均可经 `POST /api/v1/store` 帧调用（`handoff.ack` 只需 `{uri, agent_id}`）。

### 2.3 版本与血缘（M2 起可用）

- 引用 `store://.../file@v2` 或 `store://.../file@<sha256[:16]>`（内容寻址），
  缺省为最新版；`?ref=` 等价；
- `store_meta` / `store_read` 直接接受带 `@ref` 的 URI；HTTP 侧还有
  `/api/v1/versions`（版本清单 + `protected`）与 `/api/v1/manifest`（血缘）；
- commit 携带 `"manifest"`（producer / parent_uris / summary）即写入任务血缘，
  `"publish": true` 使产物全部版本永久保留（其余按 keep_last 修剪，进回收站）；
- 版本库 `.versions` 与元数据 `.nexuspouch` 是系统目录，外部 URI 机制上不可寻址。

### 2.4 ShePaw 生态内 agent

经 agent-bridge 接入的 ACP agent（Claude Code、Codex 等）由 `acp-proxy-ts`
直接注入 `store_write` / `store_read` / `store_list` 工具，与 MCP 等价，无需额外配置
（网关侧注入列为 M1.5 后续项；当前这些 agent 可直接用 §2.1 的 MCP 配置接入）。

### 2.5 接入示例

Claude Code / Codex / Cursor 配置与零依赖 Node 冒烟客户端：
`agent-bridge/examples/mcp/`（README.md 内有各平台接入命令）。

## 3. 引用纪律（Agent 侧必须遵守）

- **URI 是唯一机器 token**：原样引用、原样传递，不拼接、不改写、不自行构造；
- **`store_read(uri)` 单参数**：分块、缓存、大文件落盘由工具层处理；
- **`store_write(filename, content)`**：返回 URI 即完成共享（本地优先，后台镜像）；
- 引用格式保持 Markdown 链接 + 一句话描述：
  `[out.md](store://artifacts/<device>/task-41/out.md) — 一句话说明（产出者）`。

## 4. 身份、作用域与配额（M4 起可用）

agent 身份承载于 HTTP/MCP 层的 Bearer token（**不**混入 Noise 帧，协议兼容面冻结）：

| scope | 权限 |
|-------|------|
| `store:read` | 读共享分区（artifacts/files）与元数据 |
| `store:write` | 读写（含 `store:read` 全部能力） |
| `store:write:artifacts` / `store:write:files` | 按 space 收窄的写权限 |
| `store:delete` | 删除（默认不授予第三方 agent） |

- 每个 agent 有 `max_bytes` 配额与速率限制；超限返回 `quota_exceeded`（审计记录）。
- 未注册 / 已吊销 / token 与 agent 不匹配 → `acl_denied`。
- 设备信任仍优先：friend 级设备上的任何 agent → `denyUntrusted`。
- 管理：admin UI「Agents」页或 `POST /admin/api/agents` 创建（scopes + max_bytes），
  token 经 `POST /admin/api/tokens` 带 `agent_id` 绑定；MCP 可用
  `nexuspouch mcp-agent --agent <id> --root ...` 直接以该身份运行。

## 5. 空间占用

agent 产物按「空间预算」管理（详见实现方案 §8）：

- 版本库默认每文件保留 10 版，发布产物（`publish: true`）永久保留；
- 版本库按 sha256 去重（同内容单实例）；
- 卷用量 ≥80%（`stats.volume_warn`）时自动收紧版本保留与摘要/正文抽取；
- agent 可用 `store_space` 查询自己的占用与配额。

## 6. 相关文档

- 线协议与 fixture：[storage_protocol_spec.md](storage_protocol_spec.md)、`docs/storage_fixtures/`
- HTTP API：[API.md](API.md)
- 落地方案（M0-M5）：[IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md)
- 双端一致性：fixture 修改必须 Dart + Rust 测试同步全绿（见 `scripts/check_fixtures.sh`）
