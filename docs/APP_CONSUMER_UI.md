# ShePaw App 消费层 UI（最小设计）

> 状态：v0.1 已实现（2026-08-02）  
> 对应 BACKLOG **C**：设计 + 帧 + UI 切片已落地。本文仍为通道/挂点/降级权威。

## 0. 目标

让 M2–M5 能力在 App 储物袋里**可见可用**，不新开视觉体系：

| 能力 | 用户价值 |
|------|----------|
| 版本浏览 + 血缘 | 看历史版本、生产者与 parent |
| 全文搜索 | 按关键词找产物 |
| 交接通知 | 看见 `handoff.created`，ack 状态只读 |
| Agents（薄） | 可选只读列表；无 token 则外链 admin |
| 自定义空间 URI | `parseStoreUri` 接受合法自定义 space 名 |

## 1. 通道决策

App ↔ master 默认走 **Noise 配对**（`StoreService.call`），不依赖 admin Bearer。

| 能力 | 通道 | 说明 |
|------|------|------|
| 版本 / manifest / artifact.state | 已有 store 帧 | `versions.list` / `versions.read` / `manifest` / `artifact.state` |
| 搜索 | **新增** store 帧 `search` | 与 VECTOR 设计「App 走帧」一致；HTTP `/api/v1/search` 不变 |
| 交接事件 | **新增** store 帧 `events.list` | `{since, limit?, kind?}`；HTTP SSE 仍给脚本/admin |
| Agents | HTTP `/admin/api/agents` | 需 admin token；无凭证时 UI 降级为打开节点 admin |

增量 op **不升** `PROTOCOL_VERSION`（spec §12）。

### 1.1 `search` 帧

```json
{"op":"search","q":"关键词","space":"artifacts?","device":"?","state":"?","limit":50}
→ {"op":"result","data":{"query":"...","total":N,"results":[
    {"uri","space","device","path","sha256","size","state","snippet","score"}]}}
```

- ACL：同 `stats`（owner 允许；friend → `untrusted`）
- `q` 必填非空；可选过滤与 HTTP 一致；空索引 → `total: 0`
- master 为移动端且无索引时：`results=[]`（后续向量降级另议）

### 1.2 `events.list` 帧

```json
{"op":"events.list","since":0,"limit":50,"kind":"handoff.created?"}
→ {"op":"result","data":{"events":[...],"latest_seq":N}}
```

- ACL：同 `stats`
- `since`：返回 `seq > since`（升序，截断 `limit`，默认 50，上限 200）
- App 轮询：存 `latest_seq`，间隔约 5–15s；进前台补拉

## 2. UI 挂点（对齐现有储物袋）

模式：`Scaffold` + `AppBar` + `ListView` / `ListTile` / `ChoiceChip` / `storageToast` / 告警条。

| 能力 | 挂点 |
|------|------|
| 版本 + 血缘 | [StorageBrowserScreen](file:///Users/edenzou/workspace/shepaw/shepaw/lib/screens/storage_browser_screen.dart)：文件行 → 详情底栏「版本」→ 列表；「血缘」调 `manifest` |
| 搜索 | Browser `AppBar` 搜索图标 / 顶栏框；结果 ListTile → 跳路径或打开详情 |
| 交接通知 | 储物袋总览告警条 + 角标（仿 ImportRequestBus）；点进只读列表（uri / context / state） |
| Agents | NAS 页底段：有 admin token 则列表，否则「在浏览器打开 admin」 |
| 自定义 URI | `parseStoreUri` 改用 `StoreSpace.isValidSyntax` |

文案走 `AppLocalizations`（`storage_*`）。

## 3. 降级与边界

| 场景 | 行为 |
|------|------|
| master = Nexuspouch（配对成功） | 全能力经 Noise |
| App 自当 master（BACKLOG E） | 版本/搜索/事件帧 → 友好「需连接 NAS/节点」；本机 list/delete 仍可用 |
| 未配对 | 与现网一致；搜索/交接入口禁用或提示配对 |
| Agents 无 token | 不阻塞前三项；外链 `http://<host>:<port>/admin` |

**不做（本最小设计）：** 新视觉体系、SSE 长连接、在 App 内创建/吊销 agent、Dart master 补全服务端（E）。

## 4. 实现切片与验收

1. spec + fixture + Rust/Dart `search` / `events.list` + `check_fixtures` / `cargo test` / `flutter test test/storage/`
2. Browser 版本列表 + manifest
3. 搜索框
4. 交接通知轮询 + 总览徽章
5. `parseStoreUri` + Agents 薄层

每切片更新 [BACKLOG.md](BACKLOG.md) C 项状态。真机路径见 [DEVICE_TESTING.md](DEVICE_TESTING.md) / [QA_BASELINE.md](QA_BASELINE.md)。
