# 待办与预留项（Backlog）

> 状态快照：2026-08-02。记录 M0-M6 实现主线完成后的未完成 / 预留项，
> 按主题分组；每项标注状态、依赖与优先级建议。实现进展同步更新本文件。

## A. 边界设计 Step 3（明确预留，不承诺）

| 项 | 状态 | 依赖 |
|----|------|------|
| 空间级配额（每 space `max_bytes`，commit 时强制）与占用可视化 | 预留 | Step 2 已落地（space.declare + 属性模型） |
| 按空间粒度的导入授权（当前 import_grant 是空间属性，授权执行粒度仍是整体） | 预留 | 协议评审 |
| 自定义空间管理完善（配额、可视化、按空间统计） | 预留 | 上两项 |

## B. M6 目录绑定增强（已规划，待实现）

| 项 | 状态 | 依赖 |
|----|------|------|
| App 侧事件驱动 watcher（当前 Dart 端为轮询 `startPeriodic`；Rust 端 notify 已落地） | 待实现 | `watcher` 包（新依赖）或轮询保留 |
| 绑定 UI 目录选择器（当前手输路径，可换 file_selector 原生选目录） | 待实现 | 新依赖 + 平台权限 |
| rename/move 识别（保留版本历史；当前 delete+add） | 待实现 | Rust 用 inode；Dart 缺 inode 需 FFI 或 hash+path 双信号 |
| 半写保护（文件连续两次采样一致才摄取） | 待实现 | 无 |
| 忽略规则变更触发全量对账 | 待实现 | 无 |
| 扫描限速 + 单绑定文件上限（默认 50 万，可配） | 待实现 | 无 |
| Rust 原生 `clonefile` / `FICLONERANGE` FFI（当前 `cp -c` / `cp --reflink=auto`，行为等价但多一次进程调用） | 待实现（可选优化） | 平台 FFI |

## C. ShePaw App 消费层（协议已就绪，UI 未做）

> 最小设计：[APP_CONSUMER_UI.md](APP_CONSUMER_UI.md)。App 通道为 store 帧
> `search` / `events.list`（§2.11），不依赖 admin token。

| 项 | 状态 | 依赖 |
|----|------|------|
| 最小 UI 设计文档 | ✅ | APP_CONSUMER_UI.md |
| store 帧 `search` / `events.list`（双端） | ✅ | spec §2.11 + fixture |
| 版本浏览 UI（StorageBrowserScreen 版本列表 / 血缘 manifest 入口） | ✅ | 文件行 → 版本/血缘 |
| 交接通知展示（`handoff.created`；自动 ack 已做） | ✅ | HandoffNotifyService 轮询 + 总览告警 |
| 搜索框（调 store 帧 `search`） | ✅ | Browser AppBar SearchDelegate |
| agents 列表展示（App 存储管理页接 admin API） | ✅ 薄层 | NAS 页外链节点 `/admin`（无 App 内 token） |
| Dart 自定义空间 URI 解析（`parseStoreUri` 仍严格四空间；ACL 已对齐） | ✅ | `isValidSyntax` + fixture 更新 |

## D. agent-bridge / MCP

| 项 | 状态 | 依赖 |
|----|------|------|
| acp-proxy 网关工具管线正式注入 store 工具（`store-tools.ts` 已就绪） | 待实现 | ACP SDK 工具注入挂点 |
| MCP `store_write` 透传 `context` / `to_agent` 走 handoff（M3 语义） | 待实现 | M3 已就绪 |
| MCP `store://` 资源订阅（subscribe） | 待实现（低优先） | MCP 协议 |

## E. 协议 / 双端实现缺口

| 项 | 状态 | 依赖 |
|----|------|------|
| Dart master 侧服务端：App 自己当 master（loopback）时 versions / handoff / 自定义空间的服务端逻辑 | 待决策（双实现架构固有尾巴） | 是否值得在 Dart LocalStore 补实现 |
| Windows 平台支持（README 限定 macOS/Linux） | 待决策 | 平台适配 |
| versions 保留策略管理页 / 发布产物可视化 | 待实现（低优先） | M2 |

## F. 运维 / QA

| 项 | 状态 | 依赖 |
|----|------|------|
| 真机集成基线 QA（配对/同步/版本/交接/检索，见 QA_BASELINE.md 手工清单） | 部分：自动化绿（cargo 69 / flutter protocol 16）；curl search/versions 冒烟过；§1–5 手工待真机 | 需 App + 局域网 |
| agent-bridge vitest 在 Node 24 下启动失败（vite 兼容问题） | ✅ 已缓解（vite ^5.4 + `.nvmrc` 钉 Node 20；store-tools mock 路径修复） | 曾误诊为启动失败；现为可选环境对齐 |
| 18787 旧 nexuspouch 实例数据根目录确认（此前误杀，如需恢复） | 待确认 | 用户原启动命令 |

## G. 产品层（聊过，未成文/未实现）

| 项 | 状态 | 依赖 |
|----|------|------|
| backups「快照管理薄层」：节点侧快照注册表 + 完整性校验 + 可选节点侧保留 | 设计讨论过，未落文档/未实现 | 边界设计 Step 2 后 |
| 客户端 profile 细则收敛（ShePaw 业务字段散在 App 实现里） | 部分完成（CLIENT_PROFILES.md 已建） | 无 |

## H. 向量搜索（已定稿设计，待实现）

| 项 | 状态 | 依赖 |
|----|------|------|
| P1：embedder 抽象（默认本地 ONNX 小模型）+ embeddings 表 + 帧/HTTP/MCP `semantic` 查询 + 手机 master 降级语义 | 待实现 | 设计文档 VECTOR_SEARCH_DESIGN.md |
| P2：`memory` 空间 profile + 蒸馏记忆写入约定 + 管理页 | 待实现 | P1 |
| P3：FTS5+向量 RRF 混合排序 + 重嵌入策略 + 规模评估（HNSW 与否） | 待实现 | P1/P2 |

设计要点：向量能力绑定 master（PC/NAS）；手机经 store 帧查询、不做本地向量；
默认本地 embedding 保隐私；`memory` 空间与血缘/版本/交接自动衔接。

## 优先级建议

| 优先级 | 项 | 理由 |
|--------|----|------|
| P0（尽快） | F：真机 QA（vitest 已缓解） | 收尾稳定性，避免环境债 |
| P1（产品价值） | C：App 消费 UI（版本/搜索/交接通知） | 让 M2-M5 能力对用户可见 |
| P2（完整性） | B、D：绑定增强、MCP handoff 透传 | 完善 M6 与生态入口 |
| P3（需决策） | E：Dart master 服务端、Windows | 架构取舍，先讨论再投入 |
| P4（不承诺） | A、G：空间配额、快照薄层 | 预留，等真实需求 |
