# Agent 接入指南（Nexuspouch）

Nexuspouch 定位为异构 AI agent 之间**本地优先的产物交接与记忆总线**：任何 agent
拿到一个 `store://` URI，就能读写、校验、追溯产物，且数据默认不出用户自己的硬件。

> 里程碑状态：M0（协议 v4.2 契约 + 本文档）。M1 提供 MCP 服务器（`nexuspouch mcp`）；
> M2 版本化 URI 与血缘；M3 交接/事件；M4 agent 身份与配额；M5 检索。能力按里程碑逐步接通，
> 本文档描述的目标形态与当前可用面分开标注。

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

## 2. 目标形态（M1 起）

### 2.1 MCP 服务器（`nexuspouch mcp`）

任何支持 MCP 的 agent（Claude Code、Codex、Cursor、本地框架）通过配置即接入：

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

暴露的工具（与 HTTP API 一一对应）：

| 工具 | 作用 |
|------|------|
| `store_write` | 写文件，返回 `store://` URI |
| `store_read` / `store_read_chunk` | 读文件 / 分块读 |
| `store_meta` / `store_list` | 元数据 / 列目录 |
| `store_search` | 检索（M5 接通） |
| `store_watch` | 事件订阅（M3 接通） |
| `store_space` | 各块空间占用（空间治理） |

### 2.2 ShePaw 生态内 agent（M1）

经 agent-bridge 接入的 ACP agent（Claude Code、Codex 等）由 `acp-proxy-ts`
直接注入 `store_write` / `store_read` / `store_list` 工具，与 MCP 等价，无需额外配置。

## 3. 引用纪律（Agent 侧必须遵守）

- **URI 是唯一机器 token**：原样引用、原样传递，不拼接、不改写、不自行构造；
- **`store_read(uri)` 单参数**：分块、缓存、大文件落盘由工具层处理；
- **`store_write(filename, content)`**：返回 URI 即完成共享（本地优先，后台镜像）；
- 引用格式保持 Markdown 链接 + 一句话描述：
  `[out.md](store://artifacts/<device>/task-41/out.md) — 一句话说明（产出者）`。

## 4. 身份、作用域与配额（M4 落地）

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
