# 客户端 Profile（Client Profiles）

> 状态：v0.1（2026-08-02，Step 1 文档化）
> 定位：协议通用层之上的业务约定。**Nexuspouch 节点不感知这些约定**——
> 由客户端遵守并在客户端侧校验。边界判定见
> [SPACE_BOUNDARY_DESIGN.md](SPACE_BOUNDARY_DESIGN.md)。

## 0. 判定规则

新约定出现时问一句：「节点需要理解它才能正确执行吗？」

- **不需要** → 写进本文件（客户端 profile），协议层零改动；
- **需要** → 先进协议规范评审（双端 + fixture 同步），再落实现。

## 1. Profile 模板

```json
{
  "name": "<client-name>",
  "version": "1",
  "spaces": [
    {
      "name": "<space>",
      "visibility": "shared | private",
      "encryption": "client | none",
      "retention": "keep_last | gfs | none",
      "import_grant": "allowed | denied",
      "conventions": ["<convention-id>"]
    }
  ],
  "conventions": [
    {
      "id": "<convention-id>",
      "scope": "<space>",
      "description": "客户端遵守的内容约定，节点不校验"
    }
  ]
}
```

## 2. ShePaw Profile（首个实例）

### 2.1 空间用法

| space | 用途 | 绑定约定 |
|-------|------|---------|
| `artifacts` | Agent 产物（`<task_id>/<file>`） | 产物 URI 语义（§2.2.4） |
| `files` | 用户文件 | 无特殊约定 |
| `attachments` | 聊天附件 | hash 寻址（§2.2.2） |
| `backups` | DB 快照 + 设备身份 | 快照格式（§2.2.1）、GFS（§2.2.3） |

### 2.2 约定明细

#### 2.2.1 快照格式（scope: backups）

```text
<device_id>/backups/<snapshot-ts>/
├── manifest.json     # device_id, created_at, app_version, schema_version,
│                     # db_sha256, attachments: [hash...], 完整哈希树
├── db.sqlite.enc     # VACUUM INTO 一致性快照，XChaCha20-Poly1305
└── identity.enc      # 设备身份（Noise 密钥对），随快照加密
```

- 加密密钥 = `KDF(主密码, 设备 salt)`，**离开本机前必须已加密**；
- 附件不进快照，按 hash 引用，恢复时惰性拉回；
- 换机恢复 = App 侧流程：list → read → 本机解密 → 校验哈希树 → 全量替换。

#### 2.2.2 附件 hash 寻址（scope: attachments）

- 文件以 `sha256` 命名存储（`attachments/<hash>`），同内容单实例；
- 快照 manifest 只引用 hash，不打包正文。

#### 2.2.3 GFS 保留（scope: backups）

- 默认 daily 快照，GFS 保留 7 日 / 4 周 / 12 月；
- **剪枝在快照属主设备本机执行**（`ScheduledSnapshotService` + `selectGfs`），
  删除经同步队列镜像到 master——不是 master 代管；
- 与节点再保护包（`.system/reprotect/`）隔离，互不修剪。

#### 2.2.4 产物 URI 语义（scope: artifacts）

- `store://artifacts/<device>/<task_id>/<file>`：`task_id` 是**业务层**的目录组织约定，
  对节点只是不透明路径段；
- Agent 引用纪律：原样传递 URI，不拼接、不构造（docs/AGENTS.md §3）。

#### 2.2.5 告警（App 侧）

- 距上次成功快照（或启用锚点）超过 3 天 → 显著告警（按墙钟）；
- 未同步队列 ≥200MB、master 卷 ≥80% → 存储页告警。

## 3. 新增 Profile 或约定的流程

1. 确认不违反协议通用层（保留命名空间、ACL 属性、写收敛）；
2. 按模板补充 profile 段 + 约定明细；
3. 双端无协议改动即合入；若需要节点新能力 → 回协议评审。
