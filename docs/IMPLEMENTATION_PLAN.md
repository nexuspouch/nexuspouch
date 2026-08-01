# Nexuspouch Agent 协作落地实现方案

> 版本：v0.1（2026-08-01）
> 定位：把 Nexuspouch 从「本地存储节点」强化为「异构 AI agent 之间本地优先的产物交接与记忆总线」。
> 对标心智：Git 面向人的协作（对象模型 / 分布式对账 / 可验证 / 工具生态）；Nexuspouch 面向 agent 的协作（内容寻址 / 交接 / 血缘 / MCP 生态）。

## 0. 目标与落地原则

### 0.1 要解决的五个核心问题

| # | 问题 | 对应战役 |
|---|------|---------|
| 1 | 第三方 agent 接入摩擦大，没有行业通用入口 | M1 MCP 桥 |
| 2 | 产物只有路径寻址，无版本/内容寻址，agent 无法可靠引用 | M2 内容寻址 + 版本化 URI + manifest |
| 3 | 只有「能存能读」，没有「交接 + 确认 + 血缘」协作语义 | M3 交接协议与事件语义 |
| 4 | 信任停留在设备级，不敢让陌生 agent 安全接入 | M4 Agent 身份、作用域与配额 |
| 5 | 存储是文件树，不是 agent 能「想起」的地方 | M5 索引 / 检索 / 摘要 |
| 6 | 产物/版本/索引/事件日志让空间不可控，agent 会失去信任 | 空间占用治理（横切，§8） |

### 0.2 落地原则（写进每个里程碑的验收）

1. **协议先行、fixture 先行**：先改 `docs/storage_protocol_spec.md` + `docs/storage_fixtures/`，再写实现；延续 Dart/Rust 双端共享 fixture 的做法。
2. **增量兼容，不破坏 v4**：新增 op / 新增可选字段 / 新增 endpoint 为主；`PROTOCOL_VERSION` 只在破坏性变更时 +1。
3. **机制优先于文档**：安全与一致性靠代码机制保证（如 `.versions` 用点前缀目录天然拒绝外部路径访问），不靠「约定」。
4. **最小依赖**：新增依赖逐个评审；搜索默认走 SQLite FTS5（bundled），摘要默认走外部 hook，不内置模型推理。
5. **每个里程碑可独立交付**：有 API、有测试、有文档、有验收清单，不欠跨阶段的技术债。

### 0.3 优先级：先 ShePaw 一体化，再通用化

本方案分两步走，顺序不可颠倒：

1. **第一步（本方案主体）：ShePaw 丝滑对接。** agent 能力的默认用户是 ShePaw 生态内的 agent（App 内置 Agent、经 agent-bridge 接入的 ACP 代理、群组编排中的多端协作）。所有新能力的第一消费方是 ShePaw App，验收以「App 内真实可用」为准。
2. **第二步（通用化，设 gate 再放开）：** 协议稳定、双端实现全绿、SDK/文档/接入示例齐备后，再把同一套能力对第三方 agent 全量开放（MCP 是入口，但通用化不改变协议本身）。

由此派生四条硬约束：

- **兼容面冻结**：Noise 握手、`store.*` 帧、共享 fixture 是红线，任何里程碑不得破坏 App 现有行为；
- **每个里程碑都含 ShePaw 侧对接项与双端验收**，不出现「Rust 已实现、App 用不上」的孤岛能力；
- **新 op 必须双端对齐**：Dart 侧同步实现，或明确降级（`bad_op` + UI 提示），不允许静默不一致；
- **通用化 gate**（全部满足才启动）：协议文档 v4.2+、Dart/Rust fixture 全绿、MCP 三端示例可用、`docs/AGENTS.md` 发布。

---

## 1. 总体路线图

| 里程碑 | 主题 | 核心交付 | 预估工期 |
|--------|------|---------|---------|
| M0 | 协议固化 + ShePaw 集成基线 | 协议 v4.2 修订、URI 规范、fixture 扩充、App 现有链路基线验收 | 2-3 人日 |
| M1 | MCP 桥 | `nexuspouch mcp` 子命令 + 3 个 agent 接入示例 | 5-8 人日 |
| M2 | 内容寻址与版本化 | `@sha256` URI、`.versions`、manifest、versions API | 10-14 人日 |
| M3 | 交接与事件语义 | 产物状态机、handoff op、持久化事件日志、ack | 8-10 人日 |
| M4 | Agent 身份与配额 | agents 注册表、token 绑定、ACL 扩展、配额 | 6-8 人日 |
| M5 | 索引/检索/摘要 | FTS5 索引、`/search`、摘要 hook、`store_search` MCP 打通 | 8-10 人日 |

**每个里程碑都含 ShePaw 侧对接与双端验收（约束见 §0.3）**；空间占用治理（§8）为横切项，随 M2-M5 逐步落地。

**执行顺序说明**：M1 先做，是因为它让 ShePaw 生态内已接入的 ACP agent 第一时间获得 store 工具（生态杠杆），且不依赖 M2-M5 的语义（先用现有路径 URI）；M2 是语义内核，决定后面所有 URI/数据格式，紧接其后；M3 依赖 M2 的版本与 manifest；M4 可与 M3 并行（不同模块）；M5 依赖 M2 的 commit 钩子。

---

## 2. M0：协议固化与公开（前置，3 人日）

### 目标

让第三方开发者能够只读文档就实现 store 客户端，为 M1 的 MCP 桥和未来的生态打底。

### 任务

- [ ] `docs/storage_protocol_spec.md` 升级为 v4.2：
  - 增补 URI 规范一节（含 M2 的 `@sha256` 语法，先行定稿，实现可后置）；
  - 明确「op 集可扩展、未知 op 返回 `bad_op`」的版本兼容策略；
  - 明确 agent 身份在 HTTP/MCP 层的承载方式（token 绑定 agent_id，M4）。
- [ ] `docs/storage_fixtures/` 扩充：
  - `version_cases.json`：`@sha256` 解析、短哈希前缀冲突、`@v<N>` 别名、伪造版本引用；
  - `agent_acl_cases.json`：agent 作用域越权、配额超限、未注册 agent、token 与 agent 不匹配。
- [ ] 在 README 增加「Agent 接入」章节入口；提供 `docs/AGENTS.md` 说明 M1 的接入方式。
- [ ] CI 增加 fixture 校验：任何 fixture JSON 修改必须同时更新 Dart 与 Rust 两侧测试（沿用现有双端全绿要求）。
- [ ] ShePaw 集成基线：桌面/真机跑通「配对 → 同步 → browse → 回收站 → 管理页」现有全链路，固化为回归用例（后续里程碑对照此基线）。
- [ ] 双端 fixture 对齐工具链：任何 fixture 修改自动触发 Dart + Rust 两侧测试（App 仓库侧接入 CI）。

### 验收

- 协议文档可从零实现客户端（Dart 与 Rust 各一实现通过全部 fixture）。
- `PROTOCOL_VERSION` 仍为 4，无破坏性变更。
- 现有 ShePaw 集成链路全绿（M0 结束时建立基线，后续里程碑不得破坏）。

---

## 3. M1：MCP 桥（最高杠杆，5-8 人日）

> 状态：✅ 已落地（本分支）——`nexuspouch mcp` stdio 服务器（8 个工具 + 资源）、
> SDK 扩展、agent-bridge 接入示例（Claude Code / Codex / Cursor / Node 冒烟客户端）、
> 端到端测试与真实守护进程冒烟验证。

### 3.1 目标

让任何支持 MCP 的 agent（Claude Code、Codex、Cursor、本地框架）装一个包即可把 Nexuspouch 作为其工件/记忆/交接层。

### 3.2 形态决策

在 Nexuspouch 二进制内新增 `nexuspouch mcp` 子命令（stdio MCP server），复用 `nexuspouch::sdk::Client` 调本机 `/api/v1`：

```bash
nexuspouch mcp --root ./data --token <scoped-token>
# 或由 agent 配置直接启动
```

> 选型理由：Rust 实现零额外运行时、随二进制分发、可复用现有 API 与鉴权；agent-bridge 侧只提供「配置示例」，避免双份协议逻辑。

### 3.3 MCP 能力面

**Tools（首版）**

| 工具 | 参数 | 行为 | 对应现有 API |
|------|------|------|-------------|
| `store_write` | `filename`, `content`, `space?=artifacts`, `task?` | 写文件，返回 `store://` URI | `POST /api/v1/store` (write.begin/chunk/commit) |
| `store_read` | `uri` | 返回文件内容（>512KB 截断并提示用 `store_read_chunk`） | `GET /api/v1/read` |
| `store_read_chunk` | `uri`, `offset`, `length` | 分块读 | `GET /api/v1/read` |
| `store_meta` | `uri` | 返回 size/sha256/mtime/kind | `GET /api/v1/uri/resolve` |
| `store_list` | `space?`, `device?`, `path?` | 列目录 | `GET /api/v1/list` |
| `store_search` | `q`, `space?`, `limit?` | M5 之前返回 `not_implemented` 说明，M5 接通 | `GET /api/v1/search` |
| `store_watch` | `prefix?`, `since?` | 返回最近事件，后续 SSE 接通（M3） | `GET /api/v1/events/recent` |

**Resources（首版）**

- `store://` 前缀资源：让 MCP 客户端可直接把 `store://...` 作为资源引用（M2 之前指向现有 URI）。

### 3.4 实现落点

- 新增 `src/mcp/mod.rs`：MCP JSON-RPC 2.0 over stdio（用 `serde_json` 手工实现，不引重型 SDK；若后续需要可评估 `rmcp`）。
- `src/main.rs`：增加 `mcp` 子命令（clap），复用 `--root` / token 解析。
- `src/sdk.rs`：补充 `list()`、`store_op()` 封装（`read` 已有，缺 list 公开方法）。
- `agent-bridge/`：新增 `examples/mcp/`，提供 Claude Code（`.mcp.json`）、Codex（`AGENTS.md` 或 mcp 配置）、Cursor 三个接入示例。

### 3.5 测试

- `src/mcp/mod.rs` 单元测试：工具参数校验、错误映射（`not_found` → MCP error code）。
- 集成测试：起真实 `Local` + axum server，驱动 MCP handler 完成「写 → 读 → 校验 sha256」闭环。
- 验收：Claude Code 或 Codex 通过 MCP 配置直接 `store_write`/`store_read` 成功。

### 3.6 ShePaw 侧对接（第一步的主战场）

- **agent-bridge ACP 代理集成 store 工具**：`acp-proxy-ts` 增加 `store_write` / `store_read` / `store_list` 三个工具（内部走 Nexuspouch HTTP API 或 MCP），让经 ShePaw 接入的任何 ACP agent（Claude Code、Codex 等）开箱即用；
- **当前落地形态（M1）**：agent 侧直接挂 `nexuspouch mcp` MCP 配置（`examples/mcp/`）；网关侧原生工具注入列为 M1.5。
- **ShePaw 桌面端配置示例**：`examples/mcp/` 提供 ShePaw desktop 侧配置；App 内置 Agent 的工具层（`artifact_service`）本期不动，继续用现有 store 帧；
- **验收**：ShePaw 内一个远程 ACP agent 调用 `store_write` 产出产物 → 手机 App「存储空间」页立即可见（经 master 镜像）。

## 4. M2：内容寻址与版本化 URI（语义内核，10-14 人日）

### 4.1 URI 规范（先行定稿）

```
store://<space>/<device>/<relpath>@<ref>
```

- `<ref>` 两种形式，哈希为规范形式：
  - `@<sha256[:16]>`：内容寻址（至少 16 hex，服务端按前缀解析，前缀冲突返回 `ambiguous_ref`）；
  - `@v<N>`：顺序别名（由版本索引解析，`v1` 为首次 commit）。
- 无 `@ref` = 当前最新版本（现有行为不变）。
- `?ref=` 查询参数作为等价写法（兼容 URL 解析）。
- `uri.rs` 解析结果新增字段：`ref_kind: Hash|Seq|Latest`、`ref_value: String`。

### 4.2 版本存储布局

```
<root>/.versions/<device>/<space>/<relpath>/
├── <sha256>            # 旧版本文件本体（不可变）
├── <sha256>.meta.json  # {original_path, created_at, producer, superseded_by}
└── index.json          # {"versions": [{"v":1,"sha256":"..."}, ...]}
```

- `.versions` 是点前缀目录：现有 `normalize_path` 天然拒绝外部帧访问，机制上保证版本库不可被普通读写触碰。
- commit 时若目标已存在：旧文件先迁入 `.versions`（幂等，hash 已存在则跳过），再落新文件；被覆盖 ≠ 删除，`versions.list` 可见。
- GC：版本保留策略见 §8（默认 keep_last=10、发布产物永久保留；`delete` 后才进 `.recycle`，避免误删 agent 引用的历史）。

### 4.3 manifest（血缘载体）

每个任务目录（`<space>/<task>/`，即 space 下第一层目录）自动维护：

```json
// <space>/<task>/.nexuspouch/manifest.json
{
  "task_id": "task-41",
  "device_id": "aaaaaaaaaaaaaaaa",
  "producer": {"agent_id": "a-xxx", "model": "claude-sonnet-4", "tool": "nexuspouch-mcp/0.1"},
  "parent_uris": ["store://artifacts/bbbbbbbbbbbbbbbb/task-40/out.json"],
  "created_at": 1721300000000,
  "files": [{"path": "x.py", "sha256": "...", "size": 4096, "mtime": 0}],
  "summary": null,
  "state": "published"
}
```

- 由 commit 携带的 `"manifest"` payload（producer / parent_uris / summary）与服务器自动收集的 `files` 合并生成；`.nexuspouch` 同样为点前缀目录，外部 list/read 不可见，仅内部 `manifest` op 可读。
- 这是「agent 版 commit message + blame」，机器可解析、可校验（文件哈希可重算比对）。

### 4.4 协议与 API 新增

| 新增 op / endpoint | 说明 |
|--------------------|------|
| `versions.list`（帧） | `{space, device, path}` → 版本列表（v 号、sha256、mtime、producer） |
| `versions.read`（帧） | `{space, device, path, ref}` → 指定版本内容 |
| `GET /api/v1/versions?uri=` | HTTP 版 |
| `GET /api/v1/manifest?uri=` | 读任务 manifest |
| `commit` payload 扩展 | 可选 `"manifest": {...}`、`"publish": true`（发布即不可变） |

### 4.5 实现落点

- `src/uri.rs`：解析 `@ref` / `?ref=`，新增测试（含前缀冲突、非法 hash、`@v0` 越界）。
- 新增 `src/store/versions.rs`：迁移旧版本、版本索引、`versions.list/read`。
- 新增 `src/store/manifest.rs`：manifest 合并/读取/校验。
- `src/store/write.rs`：commit 流程插入版本迁移 + manifest 更新钩子（`commit` 返回值附带新版本号）。
- `src/store/mod.rs`：op 分发注册新 op。
- `src/api/mod.rs`：新 endpoint + fixture 对照测试。
- ShePaw 侧（`shepaw/lib/storage/`）：`versions.list` / `versions.read` 帧实现 + StorageBrowserScreen 显示版本数、版本详情、血缘（manifest）入口。

### 4.6 验收

- 写同一路径 3 次 → `versions.list` 返回 3 个版本；`read@<hash>` 与 `read@v2` 均能取回对应内容。
- 外部帧尝试读写 `.versions` / `.nexuspouch` → `bad_path`，双端 fixture 全绿。
- 迁移后的旧数据（无版本记录）视为 `v1`，`@v1` 与最新引用等价。
- ShePaw App 能浏览/读取指定版本，`@v<N>` 与 `@sha256` 引用在 App 内可点击打开。

---

## 5. M3：交接与事件语义（8-10 人日）

### 5.1 产物状态机

```
draft ──commit──> committed ──publish──> published ──ack──> acked
                     │                        │
                     └──supersede─────────────┘──> superseded
```

- 状态存于任务 `manifest.state` 与 `.nexuspouch/state.json`。
- `commit` 默认 `committed`；带 `"publish": true` 或写 `artifacts` 时默认 `published`（Agent 产物默认可引用）。
- 覆盖同路径旧版本 → 旧版本 `superseded`（保留在 `.versions`，可读不可改）。

### 5.2 交接操作

| 新增 op | payload | 语义 |
|---------|---------|------|
| `handoff.create` | 同 commit + `"to_agent"`（可选）+ `"context"`（任务描述/验收标准） | commit + 写 manifest.context + 发 `handoff.created` 事件 |
| `handoff.ack` | `{uri, agent_id}` | 消费方确认；状态 → `acked`；事件 `handoff.acked` |
| `artifact.state` | `{uri}` | 查状态 + 血缘（parent_uris / producer / state / acked_by） |

- ack 记录持久化于 `.nexuspouch/acks.json`（`{uri, agent_id, ts_ms}`），幂等（重复 ack 返回成功）。
- 交接上下文由 MCP `store_write` 的 `task`/可选 `context` 参数透传。

### 5.3 事件系统增强

- `StoreEvent` 增加 `seq: u64`（单调、持久化分配）与 `agent_id: Option<String>`。
- 新增 `.system/events.jsonl` 持久化事件日志（复用 `AuditLog` 的追加式写入模式），供离线 watcher 重放。
- `GET /api/v1/events?since=<seq>`：先重放日志中 `seq > since` 的历史事件，再切换实时 SSE；`events/recent` 保留。
- 广播容量 `BROADCAST_CAP`（256）对 watcher 偏小，方案：用 `since` 重放兜底，文档化语义即可。

### 5.4 实现落点

- `src/events.rs`：seq 分配、JSONL 持久化、since 重放。
- 新增 `src/store/handoff.rs`：状态机 + handoff ops + ack 存储。
- `src/store/write.rs` / `src/store/manifest.rs`：publish/supersede 钩子。
- `src/api/mod.rs`：`/events?since=`、`/artifact/state`（或并入 `/uri/resolve` 返回 state）。
- `src/mcp/mod.rs`：`store_watch` 接通（先返回 `since` 后的历史事件）。
- ShePaw 侧：`handoff.ack` 接入 `RemoteReadService`（跨端拉取成功后自动 ack）+ App 通知展示 `handoff.created`。

### 5.5 验收

- 写端 publish → 读端 watch 收到 `handoff.created` → ack → 写端 `artifact.state` 显示 `acked`。
- watcher 掉线期间产生的事件，`since` 重放不丢、不重复。
- 双端 fixture 增补 handoff 用例（伪造 ack、重复 ack、越权 ack 他人产物）。
- ShePaw 场景：手机 Agent 产出 → 桌面 Agent watch 收到事件 → 拉取 → 自动 ack → 手机端状态显示 `acked`。

## 6. M4：Agent 身份、作用域与配额（6-8 人日）

### 6.1 数据模型

```json
// .system/agents.json
{
  "agents": [
    {
      "id": "a-8f3c",
      "name": "claude-desktop",
      "created_ms": 1721300000000,
      "scopes": ["store:read", "store:write:artifacts"],
      "quota": {"max_bytes": 1073741824, "bytes_used": 0},
      "rate": {"tokens_per_sec": 10, "burst": 20},
      "status": "active"
    }
  ]
}
```

### 6.2 作用域设计

- 在现有 token scope（`read`/`write`/`admin`）之上增加细分 scope：
  - `store:read` / `store:write`（通用）；
  - `store:write:artifacts` / `store:write:files`（按 space 收窄）；
  - `store:delete`（默认不给第三方 agent）。
- Token 增加可选 `agent_id` 绑定：现有 scoped token 是 `np_<hex>`，扩展存储字段即可（`auth_tokens.rs`）。
- **边界切分**：agent 身份是 HTTP/MCP 层概念（token 认证后解析）；Noise peer 帧仍以设备身份为准，本期不混入 agent 字段，避免破坏与 App 的兼容面。

### 6.3 ACL 与配额强制

- `protocol::check_acl` 增加 agent 维度入口：HTTP 请求在 `AuthConfig` 解析 token 后，若绑定 agent：
  1. agent 必须存在且 `status == active`；
  2. 操作 space/op 必须在 scopes 内（否则 `acl_denied` + 审计）；
  3. 写操作前检查 `bytes_used + 本次 size <= max_bytes`（超限 `quota_exceeded`）；
  4. 简单令牌桶限速（进程内即可）。
- 审计条目增加 `agent_id` 字段；admin UI 新增「Agents」页：列表 / 创建 / 吊销 / 配额重置 / 按 agent 审计。

### 6.4 实现落点

- 新增 `src/agents.rs`：注册表读写、配额记账、令牌桶。
- `src/auth_tokens.rs`：`ApiToken` 增加 `agent_id`；create 接口接受 `--agent`。
- `src/protocol.rs`：`check_acl` 扩展（新增 agent 作用域 / 配额参数）。
- `src/admin/mod.rs` + `src/admin/ui.html`：agents 管理页。
- `src/audit.rs`：`AuditEntry` 增加 `agent_id`。
- `src/mcp/mod.rs`：`nexuspouch mcp --agent <id> --token <t>` 绑定身份。
- ShePaw 侧：App「存储空间」管理页接入 agents 列表/配额展示（复用 admin API）。

### 6.5 验收

- 创建只读 agent token → `store_write` 返回 `acl_denied`；写入超配额 → `quota_exceeded`；审计可按 agent 过滤。
- fixture `agent_acl_cases.json` 双端全绿。
- 既有设备级鉴权（Noise / loopback）行为不变，回归测试通过。
- ShePaw 管理页可见 agent 列表、配额与越权审计。

---

## 7. M5：索引 / 检索 / 摘要（8-10 人日）

### 7.1 索引

- 依赖：`rusqlite`（bundled SQLite）新建 `<root>/.system/index.db`，启用 FTS5。
- 表：`files(uri, space, device, path, sha256, size, mtime, task, state)` + `files_fts(title, body)`。
- 钩子：commit（含覆盖与版本迁移）与 delete 时更新索引（放在 `src/store/write.rs` / `browse.rs` 的既有事件点，与 EventBus 发布并列）。
- 索引内容：路径 + manifest 的 producer/parent_uris/summary；正文抽取仅对文本类扩展名（md/txt/json/log）且 ≤1MB，防止把二进制灌进索引。
- 重建命令：`nexuspouch index rebuild`（admin API `POST /admin/api/index/rebuild` 亦可）。

### 7.2 检索 API

```
GET /api/v1/search?q=<query>&space=&device=&state=&limit=50
→ {"results": [{"uri", "path", "space", "device", "sha256", "size", "state",
                "snippet", "score"}], "total"}
```

- `store_search` MCP 工具接通此 endpoint（M1 预留）。

### 7.3 摘要（可选能力，默认关闭）

- 机制：`NEXUSPOUCH_SUMMARY_URL`（OpenAI 兼容端点）+ `NEXUSPOUCH_SUMMARY_TOKEN`，或 `NEXUSPOUCH_SUMMARY_CMD <path>` 外部命令 hook；commit 后异步调用，结果写入 `manifest.summary`。
- 无配置则 `summary: null`，索引/检索不受影响。
- 目的：让 agent 用「语义查询」而不是「全量拉历史」找记忆；不内置推理引擎，保持无头节点轻量。

### 7.4 实现落点

- 新增 `src/store/index.rs`（rusqlite 封装 + 钩子 + 重建）。
- `src/api/mod.rs`：`/search`。
- `src/mcp/mod.rs`：`store_search` 接通。
- `Cargo.toml`：新增 `rusqlite = { version = "0.32", features = ["bundled"] }`（版本以当前工具链可编译为准）。
- `src/admin/mod.rs`：index 状态与重建入口。
- ShePaw 侧：StorageBrowserScreen 增加搜索框（调 `/api/v1/search`）。

### 7.5 验收

- 写入带摘要/正文的产物 → `q` 命中并返回 snippet；delete 后索引同步移除。
- 1 万文件规模下 `q` 查询 < 200ms（本地基准）。
- 无摘要配置时全链路可用（摘要缺失不报错）。
- ShePaw App 内可直接搜索产物（路径/摘要命中）。

---

## 8. 空间占用治理（横切项，贯穿 M2-M5）

### 8.1 问题

agent 是海量小文件的制造者：一个任务可能产出几十个中间产物，叠加版本库、manifest、事件日志、索引后，磁盘会以用户预期之外的速度膨胀。空间失控会直接摧毁「本地优先」的信任，是 agent 产物竞争中的硬指标。

### 8.2 空间预算模型

按 store 根目录设立可配置预算（括号内为默认值）：

| 项 | 默认 | 说明 |
|----|------|------|
| 正式区 | 不限制（用户文件） | artifacts / files / attachments / backups |
| 版本库 `.versions` | 每文件 keep_last=10（可配 0 = 不保留旧版） | 只对非发布产物生效；发布产物（`publish: true`）永久保留 |
| 回收站 `.recycle` | 30 天（现有） | 不变 |
| 事件日志 | 30 天 / 单文件 5MB 轮转 | 复用 AuditLog 追加式模式 |
| 索引 | 抽取正文 ≤1MB/文件 | 索引库随删除重建 |
| agent 配额 | M4 的 `max_bytes` | 每 agent 独立，可配置 |

### 8.3 版本库去重

- `.versions` 内文件以 sha256 命名，同一内容天然单实例（跨 relpath 共享一个 hash 文件，索引记录引用计数）；
- 正式区 + 版本库的重复内容（同 hash）可选硬链接（Unix），实现近零额外占用；
- 体积超额时写路径返回明确错误码，并与 `stats.volume_*` 联动：卷用量 ≥80%（现有 `volume_warn`）时默认收紧——自动降至 keep_last=2、暂停摘要与正文抽取，直到空间回落。

### 8.4 清理与可见性

- admin UI「空间」页：正式区 / 版本库 / 回收站 / 事件日志 / 索引各自占用，按 device/space 排序，一键清理入口（清理进 `.recycle` 可还原）；
- MCP 增加 `store_space` 工具（查询各块占用），让 agent 感知自己的占用与配额；
- 验收：100MB 产物连续写 10 个版本，卷上新增占用 ≤ 1.2GB（含去重与索引）；超预算行为全部走明确错误码而非静默失败。

### 8.5 设计决策

- D6（新增，推荐）：版本默认保留 `keep_last=10` + 发布产物永久保留（可配置），替代方案为「全量保留」——推荐前者，避免空间失控，同时保留发布产物的不可变引用能力。

---

## 9. 工程纪律

### 9.1 分支与提交流程

- 每个里程碑独立分支：`codex/m0-protocol`、`codex/m1-mcp`、`codex/m2-versioning`、`codex/m3-handoff`、`codex/m4-agents`、`codex/m5-search`。
- 合并顺序：M0 → M1 → M2 → M3 → M4 → M5；M3/M4 可在 M2 合并后并行开发（不同模块，冲突面小）。
- 每个里程碑 PR：协议/文档改动先行提交 → 实现 → 测试 → 验收清单打勾。

### 9.2 测试预算

- 新增 fixture 至少覆盖：正常路径、攻击路径（`@` 注入、点目录访问、伪造 agent）、幂等（重复 commit/ack/rebuild）。
- `cargo test` 全绿 + Dart 侧 fixture 测试全绿（双端一致性是协议的信用背书）。
- MCP 冒烟：`nexuspouch mcp` 启动后用 JSON-RPC 脚本跑「写→读→查」。

### 9.3 性能与容量

- `.versions` 与 manifest 的磁盘开销与默认策略见 §8；versions 管理项展示占用并支持手动清理（进 `.recycle`）。
- 事件日志按天轮转（复用 AuditLog 的 5MB 上限模式，改成按文件数/天数）。
- `/events` 广播容量与 since 重放性能在 M3 验收中量化。

### 9.4 与现有未提交工作区的关系

- 当前工作区有未提交改动（reprotect / snapshot_crypto / INTEGRATION.md 等）。开始 M0 前先完成这些改动的提交或暂存，避免里程碑分支混杂。

---

## 10. 竞争态势与应对（agent 产物板块）

### 10.1 竞争格局

agent 产物/记忆存储正在被多方抢：

- **MCP 生态**：filesystem server、memory server 等通用实现，接入成本低，但无设备身份、无跨设备交接、无血缘；
- **云端对象存储 SDK**（S3 / R2 等）：成熟稳定，但数据出局、无本地优先语义、账单随产物指数增长；
- **agent 记忆层**（Mem0、Letta 等）：解决「会话间记忆」，但不解决「多设备/多 agent 的产物交接」；
- **A2A / 未来行业标准**：可能定义通用工件协议，应主动对齐而非对抗；
- **Git 系产物库**：工具链成熟，但模型不适合海量小文件与二进制，无回收站/配额/镜像语义。

### 10.2 我们的差异点（按强度排序）

1. **身份绑定**：产物目录 = 设备/agent 身份，ACL 机制化，跨设备引用自带信任语义——MCP filesystem server 与云端 SDK 都没有；
2. **交接 + 血缘**：M2/M3 的版本、状态机、ack、parent_uris 是别人没有的协作语义；
3. **本地优先 + 空间经济性**：数据不出家 + §8 空间治理，直接对标「云账单恐惧」；
4. **ShePaw 深度一体化**：App 内 Agent、群组编排、ACP 代理是第一消费方，体验闭环优先于通用性。

### 10.3 竞争应对（写进各里程碑）

- **兼容而不对抗**：MCP 工具命名与 filesystem server 习惯对齐（read / write / list / meta），降低迁移成本；主动关注 A2A 与 MCP 工件规范，若出现事实标准，把 `store://` 的映射层加在协议层之下而非之上；
- **把「空间经济性」做成卖点**：M2 版本去重、M4 配额、M5 索引都要有可宣传的量化指标（写入放大、占用可视化）；
- **先占领 ShePaw 场景，再横向复制**：用 App 内的真实多 agent 工作流（手机产出 → 桌面精修 → NAS 归档）打磨协议，通用化只是把同一协议换皮开放；
- **警惕被稀释**：若 MCP 生态出现成熟的 artifacts 标准，护城河会退化为「实现质量 + ShePaw 集成」，所以 M2/M3 的交接语义与 §8 的空间治理必须领先半步。

---

## 11. 需要确认的决策点（带推荐项）

| # | 决策 | 推荐 | 备选 |
|---|------|------|------|
| D1 | MCP 实现语言/形态 | Rust 子命令 `nexuspouch mcp` | Python 版放 agent-bridge |
| D2 | 版本引用形式 | 双轨：`@sha256` 规范 + `@v<N>` 别名 | 仅顺序号 |
| D3 | 搜索后端 | SQLite FTS5（bundled） | JSON 倒排 / 外部 Meilisearch |
| D4 | 摘要生成 | 外部 LLM hook，默认关 | 首版不做 |
| D5 | 协议版本策略 | 增量扩展，`PROTOCOL_VERSION` 保持 4 | 升 v5 并做显式协商 |
| D6 | 版本默认保留策略 | keep_last=10 + 发布产物永久（可配置） | 全量保留 |

> 若对 D1-D6 有不同意见，只需改对应小节的设计，其余链路不受影响。

---

## 12. 建议排期（4-6 周节奏，1-2 人）

| 周次 | 内容 |
|------|------|
| 第 1 周 | M0 协议定稿 + M1 MCP 桥 |
| 第 2 周 | M1 收尾（三端接入示例 + 验收）+ M2 设计评审 |
| 第 3-4 周 | M2 版本化 + manifest |
| 第 5 周 | M3 交接与事件（M4 并行开工） |
| 第 6 周 | M4 收尾 + M5 索引检索 |
| 第 7-8 周 | M5 收尾、全量回归、双端 fixture 同步、发布 0.2.0 |

> ShePaw 侧实现与 Rust 侧按同一份 fixture 并行开发，工期已含双端对齐时间；空间治理（§8）随 M2（版本库）、M4（agent 配额）、M5（索引）同步落地，不额外占排期。

---

## 附录：本方案与现有代码的映射速查

| 改动点 | 文件 |
|--------|------|
| URI 扩展 | `src/uri.rs` |
| 版本存储 | 新增 `src/store/versions.rs` |
| manifest | 新增 `src/store/manifest.rs` |
| commit 钩子 | `src/store/write.rs`、`src/store/mod.rs` |
| 事件增强 | `src/events.rs`、`src/api/mod.rs` |
| 交接 | 新增 `src/store/handoff.rs` |
| Agent 注册表 | 新增 `src/agents.rs`、`src/auth_tokens.rs`、`src/protocol.rs` |
| 审计扩展 | `src/audit.rs` |
| 索引 | 新增 `src/store/index.rs`、`Cargo.toml` |
| MCP | 新增 `src/mcp/mod.rs`、`src/main.rs`、`src/sdk.rs` |
| 管理面 | `src/admin/mod.rs`、`src/admin/ui.html` |
| ShePaw 版本/血缘 | `shepaw/lib/storage/`（versions 帧、StorageBrowserScreen、manifest 入口） |
| ShePaw 交接/事件 | `shepaw/lib/storage/`（RemoteReadService ack、handoff 通知） |
| ShePaw 搜索/配额 | `shepaw/lib/screens/storage_space_screen.dart`（搜索框、agents 列表） |
| ACP 代理 store 工具 | `agent-bridge/implementations/acp-proxy-ts/`（store_write/read/list） |
| 接入示例 | `agent-bridge/examples/mcp/`（Claude Code / Codex / ShePaw desktop） |
