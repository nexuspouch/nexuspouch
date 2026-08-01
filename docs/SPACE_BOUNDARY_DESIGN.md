# Nexuspouch 通用化边界设计（Space Boundary & Generic Layer）

> 状态：设计稿 v0.1（2026-08-02）
> 定位：从「ShePaw 的存储节点」走向「agent 协作通用层」的分层边界定义。
> 关联：storage_protocol_spec.md（空间/ACL/保留命名空间）、
> IMPLEMENTATION_PLAN.md（M0-M5）、M6_FOLDER_BINDING.md。

## 0. 结论摘要

- 三层模型：**系统机制层 / 协议通用层 / 业务约定层**；
- 空间 = **属性驱动的分区**（visibility / encryption / retention / import_grant），
  现有四分区降级为「内置 profile」，新空间无需改协议代码；
- 业务约定（快照格式、附件寻址、GFS 语义、task_id 含义）下沉为**客户端 profile**，
  Nexuspouch 节点不感知；
- **reprotect 迁入 `.system/reprotect/`**，消除与用户 backups 的命名冲突；
- 分三步落地：文档先行 → `space.declare` + reprotect 迁移 → 预留扩展。

## 1. 目录归属清单（现状 → 目标）

| 层 | 目录/概念 | 现状 | 目标 |
|----|----------|------|------|
| 系统机制 | `.system/`（identity、master 指针、peers、tokens、agents、events、index、audit）、`.recycle/`、`.versions/`、`<space>/.staging/`、`<task>/.nexuspouch/` | ✅ Nexuspouch 独占 | 不变；点前缀 + 协议拒绝 |
| 系统机制 | `backups/reprotect-*` | ⚠️ 复用用户 backups 空间 | **迁移到 `.system/reprotect/`** |
| 协议通用 | 四分区名字 + ACL 矩阵 | ⚠️ 协议写死 | 改为**属性驱动的 space profile**，四分区为内置实例 |
| 业务约定 | 快照格式、附件 hash 寻址、GFS 管 DB 快照、task_id 语义 | ❌ 渗入协议/实现 | 移出，作为 ShePaw 客户端 profile 文档化 |

## 2. 三层模型

### 2.1 系统机制层（Nexuspouch 独占，永不业务化）

- `.system/`：身份、指针、配对、token、agents、事件日志、搜索索引、审计、绑定表；
- `.recycle/`：回收站；`.versions/`：版本库；`<space>/.staging/`：暂存；
- `<task>/.nexuspouch/`：任务血缘 manifest / 状态 / ack；
- 机制保证：所有 `dot` 前缀目录被 `normalize_path` 拒绝，外部帧不可寻址，
  仅内部 op 访问——这条纪律是边界的地基。

**reprotect 迁移**：镜像树再保护是系统功能，不是业务内容。现占用
`<master>/backups/reprotect-*` 与用户快照共用命名空间（靠前缀隔离，存在碰撞风险），
目标布局：

```text
<store>/.system/reprotect/<ts>/
├── manifest.json      # kind=mirror_reprotect
└── mirror.tar.enc     # XChaCha20-Poly1305，KDF 与 ShePaw 兼容
```

保留策略不变（keep_last=4，仅匹配 `reprotect-`），但不再与用户 backups 争命名空间。

### 2.2 协议通用层（Nexuspouch 拥有）

通用原语（M0-M5 已落地）：内容寻址与版本、血缘 manifest、交接状态机、FTS5 检索、
事件流、ACL、retention 原语、agent 身份/作用域/配额。

**空间属性模型（本设计的核心新增）**：

```json
{
  "name": "<space>",
  "visibility": "shared | private",
  "encryption": "client | none",
  "retention": "keep_last | gfs | none",
  "import_grant": "allowed | denied"
}
```

- `visibility` 决定 ACL 矩阵与跨端读（shared → owner 可读他端；private → 仅本端 +
  显式例外）；
- `encryption` 只声明"客户端负责加密"，节点永远不持有/处理密钥；
- `retention` 是原语（参数由调用方按内容给），不是业务策略；
- `import_grant` 控制换机导入授权粒度（迁移例外是否允许）。

**四个内置 profile（兼容不动）**：

| space | visibility | encryption | retention | import_grant |
|-------|-----------|-----------|-----------|--------------|
| artifacts | shared | none | none | allowed |
| files | shared | none | none | allowed |
| attachments | private | client | none | allowed |
| backups | private | client | gfs（客户端执行） | allowed |

新空间（如 `models`、`traces`、`datasets`）经 admin `space.declare` 声明属性后即可使用，
不需要改协议代码或双端实现。

### 2.3 业务约定层（客户端 profile，Nexuspouch 不感知）

以下内容从协议/实现中移出，作为 **ShePaw 客户端 profile** 文档化，由客户端遵守：

- 快照格式：`backups/<ts>/manifest.json + db.sqlite.enc + identity.enc`（加密密钥 =
  KDF(主密码, salt)，解密永不发生在节点上）；
- 附件 hash 寻址约定（`attachments/<hash>`）；
- GFS 作为 DB 快照保留策略的用法（属主设备本机执行，删除经同步队列镜像）；
- `store://artifacts/<dev>/<task>/<file>` 中 `task_id` 的业务含义（对节点只是不透明路径段）；
- 换机恢复、恢复前安全快照、3 天未快照告警等 App 侧流程。

**边界判定规则**：新需求出现时问一句「节点需要理解它才能正确执行吗？」——
不需要 → 放 manifest 的 `detail` 或客户端侧；需要 → 才考虑进协议通用层。

## 3. 边界控制机制

1. **保留命名空间**：`dot` 前缀机制不变（`.system/.recycle/.versions/.staging/.nexuspouch`）；
2. **空间名不是语义**：名字只是 key，行为由属性决定——协议不再对空间名做业务假设；
3. **业务约定必须走 profile 文档**：新增业务字段不得直接进协议实现；
4. **变更影响面检查表**：协议层改动 = 双端 + fixture 同步；业务层改动 = 仅客户端。

## 4. 落地步骤

### Step 1（文档化，零代码）

> 状态：✅ 已落地（本分支）——`storage_protocol_spec.md` §0.5 空间属性模型
> + `docs/CLIENT_PROFILES.md`（ShePaw 首个客户端 profile）。

- 协议规范新增「空间属性模型」章节，把四分区重述为属性的实例；
- 发布「客户端 profile」模板，ShePaw 作为首个 profile 收录（快照/附件/GFS/URI 语义）。

### Step 2（小改动）

> 状态：✅ 已落地（本分支）——`space.declare` / `space.list`（store 帧 + admin
> `/spaces` + UI 卡片）、`check_acl_with` 属性驱动 ACL、自定义空间端到端可用、
> reprotect 迁入 `.system/reprotect/`（含旧位置自动搬迁）、space profile fixture。

- admin `space.declare`（声明 name + 属性，校验命名与保留前缀）；
- reprotect 迁入 `.system/reprotect/`（含旧位置兼容读取或迁移提示）；
- fixture 增加 space profile 用例（属性 × ACL 期望）。

### Step 3（预留，不承诺）

- 空间级配额与可视化；自定义空间的管理页/API；按空间粒度的导入授权。

## 5. 对已有决策的收编

- **backups 命名问题**：用 visibility 模型回答——backups 是 `private` profile 的实例；
  "锁区"概念 = `private + encryption: client + import_grant: allowed（迁移例外显式化）`，
  不必为此改名或合并空间；
- **M6 目录绑定**：bindings 是协议通用层的摄取能力（reflink/hardlink/copy + 事件对账），
  与空间属性模型正交，绑定目标可为任意 space；
- **reprotect 冲突**：随 Step 2 迁移解决。

## 6. 验收

- 双端 fixture 全绿且新增 space profile 用例；
- 声明新空间（如 `models`）后，无需改动 Rust/Dart 协议代码即可 list/read/write；
- 节点代码不含 db.sqlite / 附件 / 快照业务逻辑（reprotect 迁走后）；
- 协议文档明确区分「系统层 / 通用层 / 业务层」三节，ShePaw profile 独立成文。
