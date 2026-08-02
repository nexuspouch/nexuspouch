# 向量搜索设计（Vector Search）

> 状态：P1 骨架已落地（2026-08-02，分支 `codex/vector-p1`）
> 核心决策：**向量能力绑定 master（常开 PC/NAS）**；手机只做客户端查询，
> 不在移动端保存模型/索引/embedding。默认本地 embedding，远程 API 仅显式可选。
>
> P1 实现备注：默认 embedder 为离线 `local-hash-v0`（特征哈希，保隐私、免下模型）；
> 配置 `NEXUSPOUCH_EMBED_URL` 走远程；`NEXUSPOUCH_EMBED=off` 强制降级 FTS5。
> ONNX bge-small 替换为后续增量。

## 0. 结论摘要

- 必要性：语义检索是 agent「记忆层」的核心——M5 FTS5 是精确匹配，向量补语义召回；
- 边界：向量 = master 能力；master 为移动设备时 `semantic` 优雅降级回退 FTS5；
- 隐私：默认本地 embedding（bge-small 级小模型，CPU 可跑），内容不出硬件；
- 通道：App 走 store 帧 `search`（Noise 配对）；HTTP / MCP 服务外部 agent，三端同一查询语义；
- 落地：P1 embedding provider + 向量列 + 语义查询；P2 `memory` 空间；P3 混合排序。

## 1. 必要性

- 记忆场景是"记得大概、忘了原话"——FTS5 需精确词，向量按语义召回；
- 与 M5 衔接：已有 manifest 摘要 + commit 钩子 + FTS5 索引，向量只是同一条链路加 embedding 列；
- 与 `memory.digest.offer`（she-network）概念闭环：agent 写蒸馏记忆 → 按语义召回；
- 前提约束：**必须默认本地 embedding**，否则破坏「数据不出硬件」的隐私叙事。

## 2. 竞品对照

| 类别 | 代表 | 强 | 弱（机会） |
|------|------|----|-----------|
| 向量数据库 | Pinecone / Qdrant / Weaviate / Milvus | 成熟、HNSW | 无设备身份/多设备镜像/交接/血缘 |
| Agent 记忆框架 | Mem0 / Letta / Zep | 记忆提取 + 语义召回 API | SaaS 隐私弱；单机；记忆与产物分离 |
| MCP memory server | 官方 memory / Chroma MCP | 接入摩擦最低 | 单机、无同步、无身份边界 |
| 本地 RAG | AnythingLLM / Jan / LM Studio | 本地 embedding 成熟 | 桌面单机，非多设备共享 store |

差异化：**身份绑定多设备 + 本地优先 + 血缘版本交接 + MCP 即插即用**的组合，
而非"又一个向量检索"。

## 3. 架构决策

### 3.1 能力归属：master 节点

- 索引与 embedding 只建在 master（`<root>/.system/vector/`），手机零向量负担
  （不存模型、不建索引、不计算 embedding）；
- **降级语义**：master 是移动设备时，`semantic: true` 返回 `vector_unavailable`
  或自动回退 keyword（响应带 `"degraded": true`），行为可预期；
- 记忆聚合地本就该是 master（跨端读权威、常开、备份/reprotect 都在节点）。

### 3.2 查询通道（三端同一语义）

| 通道 | 方式 | 使用者 |
|------|------|--------|
| store 帧 `search`（`semantic` 标志） | Noise 配对 | App（手机/桌面） |
| `GET /api/v1/search?semantic=true` | Bearer / loopback | 脚本 / 外部 agent |
| MCP `store_search`（`semantic` 参数） | MCP | 任意 agent |

### 3.3 embedding provider 抽象

```rust
trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
    fn dims(&self) -> usize;
}
```

- `LocalEmbedder`（默认）：ONNX 小模型（bge-small-zh / e5-small 级），CPU 推理；
- `RemoteEmbedder`（可选）：OpenAI 兼容 `/embeddings`，需用户显式配置；
- 配置：`NEXUSPOUCH_EMBED_MODEL` / `NEXUSPOUCH_EMBED_URL` / `NEXUSPOUCH_EMBED_TOKEN`；
- 隐私：默认本地；启用远程时文档与 UI 明示"向量出机"。

### 3.4 存储与索引

- 单条向量 ≈ 2KB（512 维 float32）；100 万条约 2GB；模型 100-400MB（一次性）；
- 索引：扩展 SQLite（`embeddings` 表：uri / chunk_index / embedding BLOB / model /
  created_ms）；小规模暴力余弦即可，P3 视规模评估 HNSW（sqlite-vec 或外部库）；
- 分块：≤512 token/块、重叠 64；优先 embedding manifest.summary（便宜）+ 正文文本块；
- 重嵌入：模型变更 / 内容变更（版本化驱动）时按 uri 重算。

## 4. `memory` 空间（P2）— 已落地

- 节点 `Local::open` 自动 `ensure_memory()`：well-known 自定义空间
  （默认 private / client / keep_last / import denied）；
- 约定见 [CLIENT_PROFILES.md](CLIENT_PROFILES.md) §2.2.6：
  `store://memory/<device>/<topic>/<ts>.md`；
- admin Spaces 列表标注 `well-known` + convention；
- 检索：`store_search(semantic: true, space: memory)` /
  帧 `search` 同参。

## 5. 混合检索（P3）

- FTS5 + 向量 RRF 融合排序；`space/device/state` 过滤沿用；
- 响应标注 `score_type: keyword|vector|hybrid` 与 `degraded` 标志。

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|------|------|------|
| P1 | embedder 抽象（本地默认）+ embeddings 表 + 帧/HTTP/MCP `semantic` 查询 + 降级语义 | 1-2 周 |
| P2 | `memory` 空间 profile + 蒸馏记忆约定 + 管理页展示 | 1 周 |
| P3 | RRF 混合排序 + 重嵌入策略 + 规模评估（是否上 HNSW） | 1 周 |

## 7. 风险与对策

| 风险 | 对策 |
|------|------|
| 模型下载/依赖 | 首次按需下载；失败退 keyword-only |
| 隐私（远程 provider） | 默认本地；远程显式开启并明示 |
| 手机作 master | 文档化 + `degraded` 标志回退 |
| 规模增长 | 100 万条内暴力余弦；更大上 HNSW |
