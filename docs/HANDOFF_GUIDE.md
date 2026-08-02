# Nexuspouch 交接说明（Handoff Guide）

> 给接手下一位开发/agent 的必读文档。目标是让接手者**用最短时间确认
> "做什么、以哪份文档为准、不能违反什么、怎样算做完"**。

## 0. 项目全景（30 秒）

- **Nexuspouch**（Rust）：本地优先的 agent 协作存储节点，master 常开部署；
- **ShePaw**（Flutter App）：客户端，本地优先写 + 镜像到 master，双端同一协议；
- **agent-bridge**：SDK/ACP 网关/MCP 示例；**channel**（Go）：跨网隧道中继；
- M0-M6 主线已落地：协议 v4.3、MCP、版本化+血缘、交接+事件、agent 身份/配额、
  FTS5 检索、目录绑定（reflink/hardlink）、空间属性模型。剩余项见 BACKLOG.md。

## 1. 必读文档与阅读顺序

按顺序读（约 20 分钟）：

1. [BACKLOG.md](BACKLOG.md) —— 唯一总清单：剩余项 A-H、状态、优先级；
2. [storage_protocol_spec.md](storage_protocol_spec.md) —— 协议权威（v4.3）；
3. [SPACE_BOUNDARY_DESIGN.md](SPACE_BOUNDARY_DESIGN.md) —— 三层边界 + 空间属性模型；
4. [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) —— M0-M5 落地记录 + 工程纪律
   （§8 空间治理 / §9 分支提交 / §10 竞争 / §11 决策）；
5. 目标能力的设计文档（见 §2 映射表）；
6. [QA_BASELINE.md](QA_BASELINE.md) + [DEVICE_TESTING.md](DEVICE_TESTING.md) —— 验收与真机路径；
7. [INTEGRATION.md](INTEGRATION.md) / [CLIENT_PROFILES.md](CLIENT_PROFILES.md) /
   [AGENTS.md](AGENTS.md) / [API.md](API.md) —— 按需查阅。

## 2. 待办项 → 权威文档映射

| 待办 | 以哪份文档为准 | 备注 |
|------|--------------|------|
| A 空间配额/按空间导入授权（Step 3） | SPACE_BOUNDARY_DESIGN.md（Step 3）+ spec §0.5 | 预留不承诺，先讨论 |
| B M6 绑定增强（rename/半写/上限/原生 reflink） | M6_FOLDER_BINDING.md §3-§6、§13 | 核心已落地，增强项 |
| C ShePaw App 消费 UI（版本/交接通知/搜索/agents） | [APP_CONSUMER_UI.md](APP_CONSUMER_UI.md) + spec §2.11 + shepaw 现有实现 | 最小设计已补；实现按该文档切片 |
| D agent-bridge/MCP（网关注入/handoff 透传/订阅） | agent-bridge 仓库 README + examples/mcp + store-tools.ts + AGENTS.md | 跨仓库 |
| E 双端缺口（Dart master 服务端、Windows） | spec v4.3 + SPACE_BOUNDARY_DESIGN.md | Dart master 服务端已决策**不实现**（master 能力归 Nexuspouch，App 仅客户端，手机 master 降级）；Windows 待决策 |
| F 运维/QA（真机、vitest Node24） | QA_BASELINE.md + DEVICE_TESTING.md；vitest 在 agent-bridge 仓库 | 真机需硬件 |
| G 快照管理薄层 | **未成文，先补设计**（建议并入 SPACE_BOUNDARY_DESIGN 或独立文档） | 只有讨论记录 |
| H 向量搜索 P1-P3 | VECTOR_SEARCH_DESIGN.md（已定稿） | 按里程碑推进 |
| I 会话历史管理 P1-P3 | SESSION_HISTORY_DESIGN.md（已定稿） | 依赖 H P1（P2 语义召回） |

## 3. 工作规则（红线）

### 协议与双端

- 任何协议行为改动（op、错误码、ACL、URI）必须：
  1. 先改 `docs/storage_protocol_spec.md` + `docs/storage_fixtures/`（双端契约）；
  2. Rust（Nexuspouch）与 Dart（ShePaw `lib/storage/store_protocol.dart`）同步实现；
  3. 双端 fixture 测试全绿（`scripts/check_fixtures.sh`）；
- `PROTOCOL_VERSION` 只在破坏性变更时 +1；增量 op/可选字段不升版本；
- `dot` 前缀目录（`.system/.recycle/.versions/.staging/.nexuspouch`）机制上不可寻址，
  不得为业务开洞；store 树内禁止符号链接。

### 测试

- Nexuspouch：`cargo test` 全绿（当前 69）+ 无编译警告；
- ShePaw：`flutter test test/storage/` 全绿（协议 16+、绑定 2、其他按既有基线）；
- 已知既有 flake：`snapshot_service_test` 恢复计数与 `scheduled_snapshot_test`
  密码变更（基线同样失败，与协议无关，勿"顺手修"）；
- agent-bridge：推荐 Node 20（`.nvmrc` / Docker）；vitest 2.1 + vite 5.4
  在 Node 22 也可启动。不要顺手升级 vitest/vite 大版本；日常可用
  `npm test` + `npx tsc --noEmit`。

### 分支与提交

- 新能力分支：`codex/<milestone>`（如 `codex/vector-p1`）；不直接提交 main；
- 每个能力：文档先行 → 实现 → 测试 → 文档状态更新 → 提交；
- 改动完成后必须更新 BACKLOG.md 对应项状态（✅/待办），README 入口同步。

### 仓库边界

- Nexuspouch（Rust）：`/Users/edenzou/workspace/Nexuspouch`；
- ShePaw（Dart）：`/Users/edenzou/workspace/shepaw/shepaw`；
- agent-bridge：`/Users/edenzou/workspace/shepaw/agent-bridge`；
- channel（Go）：`/Users/edenzou/workspace/shepaw/channel`（本期基本不动）。

## 4. 每类任务的完成定义（DoD）

| 任务类 | 完成 = |
|--------|--------|
| 文档/设计 | 文档入库 + 提交 + 相关 BACKLOG 项状态更新 |
| 协议能力 | spec + fixture + Rust/Dart 双实现 + 双端测试绿 + 契约测试绿 |
| 节点功能 | cargo test 绿 + 无警告 + 用例覆盖（正常/攻击/幂等）+ 文档更新 |
| App 功能 | flutter analyze 无 error + 相关 storage 测试绿 + UI 走查（真机清单） |
| MCP/agent 入口 | tsc 类型检查过 + 冒烟脚本可用 + AGENTS.md 更新 |
| 运维/QA | 真机 8 步路径执行 + QA_BASELINE 勾选 + 发现的问题进 BACKLOG |

## 5. 环境注意事项

- 端口 8787（节点）、18787/18789（已被清理，勿占用）；遗留进程先确认归属再杀；
- 节点启动：`cargo run -- --root ./data --listen :8787 --name dev`；
- mDNS 依赖局域网；iOS 真机需 `NSBonjourServices` 配置（见 DEVICE_TESTING.md）；
- 外部目录绑定：`--bind <external>=<space>/<folder>[:mode]`，
  `hardlink-immutable` 同卷零拷贝，`auto` 走 `cp -c`/`cp --reflink=auto`。

## 6. 建议的接手顺序

1. **P0（收尾）**：F——vitest 已缓解（钉 Node 20）；真机 QA 仍待执行；
2. **P1（产品价值）**：C——App 消费 UI 已落地（设计 + search/events 帧 + Browser/通知）；
3. **P2（完整性）**：B 半写/上限/ignore 对账与 D MCP handoff 透传已落地；
   余 rename/FFI/App watcher、网关注入仍待；
4. **P3（需决策）**：E——Dart master 服务端、Windows；
5. **P4（不承诺）**：A、G——空间配额、快照薄层（先讨论后投入）；
6. **新方向**：H——P1+P2 在 `codex/vector-p1`（ONNX bge-small-zh 默认 +
   hash 回退 + semantic 降级 + `memory` 空间）；RRF 混合仍待。
