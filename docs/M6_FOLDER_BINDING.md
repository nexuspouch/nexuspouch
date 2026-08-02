# M6 设计：本地目录绑定与摄取（Folder Binding）

> 状态：设计稿 v0.1（2026-08-01）
> 实现状态：M6a（App 目录绑定服务 + 管理页 UI）+ M6b（节点 `--bind` +
> notify 事件驱动 watcher + 60s 周期兜底）已落地（2026-08-02）；
> reflink/hardlink 摄取为后续增强（需扩展 commit 写路径）。
> 关联：[storage_space_plan.md](storage_space_plan.md)（设备目录模型）、
> [storage_protocol_spec.md](storage_protocol_spec.md) §2（写路径收敛）、
> [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md)（M0-M5 已落地）

## 0. 结论摘要

- 绑定 = **配置层映射**（外部目录 → store 路径），store 内部始终是真实文件；
- 摄取优先 **reflink > hardlink > copy**，避免双份存储，同时保住内容寻址；
- 变化感知 = **FS 事件驱动对账**：事件只是唤醒信号，sha256 对账才是正确性来源；
- 分两阶段：Phase A 桌面 App 侧绑定（复用 SyncEngine），Phase B Nexuspouch `--bind`（常开）。

## 1. 目标与非目标

### 目标

- 用户选择本地目录 → 内容出现在 `store://files/<device>/<folder>/`，不重复存两份；
- 目录内容变化由守护进程自动维护（事件驱动 + 定期对账）；
- 摄取内容自动获得 M2-M5 能力：版本化（`.versions`）、manifest 血缘、FTS5 索引、可 handoff。

### 非目标（本期明确不做）

- **双向同步**：store 侧改动写回外部目录——写权威反转 + 冲突解决，另案；
- **逻辑虚拟目录**：store 树内放置非真实文件的 bind 节点——破坏内容寻址与不可变版本；
- 跨设备"直接挂载"外部目录（外部目录必须与摄取进程同机）。

## 2. 绑定模型

绑定表持久化于 `<store>/.system/bindings.json`（仅 admin / loopback 可改）：

```json
{
  "bindings": [
    {
      "id": "b-1",
      "label": "下载",
      "external": "/Users/me/Downloads",
      "space": "files",
      "folder": "downloads",
      "mode": "auto",
      "ignore": [".DS_Store", "*.tmp", "~*", "Thumbs.db"]
    }
  ]
}
```

- 外部目录是 store 的**单向输入源**；摄取产物为 `files/<device>/<folder>/` 下的真实文件；
- 设备归属：Phase A 归属桌面设备 `device_id`；Phase B 归属节点自身 `device_id`（节点 files 空间对所有 owner 可读）；
- store 树内**不存在**指向外部的符号链接（现有 `reject_symlink_under` 纪律不变）。

## 3. 摄取机制：reflink > hardlink > copy

### 3.1 机制对比

| 机制 | 初始存储开销 | 外部就地修改是否影响 store 副本 | 适用条件 |
|------|------------|-------------------------------|---------|
| reflink（CoW 克隆） | 零拷贝（按页 CoW） | **不影响**，两侧独立快照 | 同卷；APFS `clonefile` / btrfs·xfs `FICLONERANGE` |
| hardlink | 零拷贝（同 inode） | **影响**（就地写会污染旧版本） | 同卷；仅外部"只增不改/变更即新文件"的目录 |
| copy | 双份 | 不影响 | 跨卷 / 异文件系统兜底 |

### 3.2 选择顺序（按文件）

1. 同卷且 reflink 可用 → reflink（macOS APFS、Linux btrfs/xfs）；
2. 同卷且绑定 `mode: "hardlink-immutable"`（用户显式承诺目录不可变）→ hardlink；
3. 其余（跨卷、ext4/NTFS）→ copy。

- `mode: "auto"` = reflink → copy（不自动用 hardlink，避免就地修改污染）；
- 实现提示：macOS 用 `clonefile`，Linux 用 `ioctl(FICLONERANGE)`，可封装为小函数或评估
  `reflink` crate；不引入 FUSE。

### 3.3 hardlink 的纪律

- 仅在 `mode: "hardlink-immutable"` 下启用；
- 对账以 inode + mtime 为信号：检测到原 inode 内容变化（就地写）→ 以新 inode 重新摄取，
  旧版本标记 `warning: shared inode mutated`，不静默污染版本库。

## 4. 变化感知：事件驱动对账

### 4.1 事件来源

| 平台 | 机制 | Rust | Dart（Phase A） |
|------|------|------|----------------|
| macOS | FSEvents | `notify` crate | `watcher` 包 |
| Linux | inotify | `notify` crate | `watcher` 包 |
| Windows | ReadDirectoryChangesW | `notify` crate | `watcher` 包 |

### 4.2 原则

**事件是唤醒信号，对账是事实来源。** 编辑器原子 rename、inotify 队列溢出、FSEvents
合并、休眠唤醒都可能让事件不完整——正确性不依赖事件完整性。

### 4.3 流程

1. 启动：全量扫描，建立 `{path → sha256, mtime, inode}` 快照；
2. 运行：事件 debounce（约 500ms）→ 触发该绑定目录扫描 → 与快照 diff
   （新增 / 修改 / 删除 / rename）；
3. 定期（默认 1h）：全量对账兜底；
4. diff 结果走摄取写路径（§7）；
5. 半写保护：文件连续两次采样（间隔 300ms）大小/mtime 一致才摄取。

## 5. 忽略规则

- 默认忽略：`.` 开头条目（`.DS_Store`、`.git`、`.Trash`）、`*.tmp`、`~*`、`*.swp`、
  `Thumbs.db`；
- 绑定可配 `ignore` glob；忽略规则变更 → 触发一次全量对账（纳入/剔除受影响文件）；
- 外部目录内的**符号链接一律跳过**（不跟随、不摄取），维持无链接纪律。

## 6. rename 语义

- 同卷以 inode 为主要信号识别 move；跨卷/不可靠时以"内容 hash 不变 + 路径变化"为辅；
- 识别为 move：store 内对应文件 rename 到新路径，**保留版本历史**与 manifest 记录，
  发 `artifact.renamed` 事件（`from_uri` / `to_uri`）；
- 无法识别（内容已变 / 跨卷）→ 退化为 delete + 摄取，审计记录；
- 目的：agent 引用的 `store://` URI 尽量稳定，改名可经血缘追溯。

## 7. 摄取与既有写路径 / 钩子

- 新增 op `ingest`（增量扩展，不升协议版本）：
  - payload：`{space, device, path, source, mode}`；
  - 语义：目标已存在 → 旧文件先入 `.versions`（复用现有逻辑）；外部源文件按
    §3.2 落盘到 dest；reflink/copy 校验 sha256，hardlink 读校验；
  - 落盘后更新 manifest / 索引 / handoff 状态——现有 commit 钩子全量复用；
- 只读面不变：HTTP / MCP / WebDAV 无需改动；外部工具写入 store 仍走 `store_write`，
  与绑定摄取是两条独立路径；
- 审计：ingest 事件记录 `binding_id`、外部路径、设备、模式（reflink/hardlink/copy）。

## 8. 安全

- store 树内无符号链接（现有拒绝逻辑不变）；reflink/hardlink 是普通目录项，不引入逃逸面；
- ACL / 写收敛不变：摄取以绑定设备身份（loopback owner）执行，外部帧无法指定外部路径；
- 绑定表变更仅 admin / loopback；`bindings.json` 权限同 `.system` 其他文件。

## 9. Phase A：桌面 App 侧绑定（先行）

> 状态：✅ 核心已落地——`shepaw/lib/storage/folder_binding_service.dart`
> （绑定注册表 + sha256 对账摄取 + 删除进回收站 + 忽略规则 + 轮询周期同步），
> 测试全绿；UI 目录选择器为后续项。

- Dart：`watcher` 监听用户所选目录 → 复用 `LocalStore` + `SyncJournal` + `SyncEngine`
  摄取（先 copy，后续同卷可升级 reflink）；
- UI：设置 → 存储空间 → 「绑定目录」：选目录、映射 space/folder、忽略规则；
- 设备归属：桌面设备 `device_id`；master 离线时绑定目录仍本地可写（符合本地优先）；
- 验收：选目录 → 内容出现在 files；改文件 → 手机端可见且有版本；删文件 → 回收站；
  忽略规则生效。

## 10. Phase B：Nexuspouch 节点侧 --bind（常开）

> 状态：✅ 核心已落地——`nexuspouch --bind <external>=<space>/<folder>[:mode]`
> + `.system/bindings.json` 注册表 + `bindings sync` 命令 + 60s 周期同步
> （loopback 写路径摄取，版本/索引钩子自动触发）；reflink/hardlink 为后续增强。

- 配置：`nexuspouch --bind <external>=files/<folder>[:mode]` 或 `bindings.json`；
- 实现：`notify` + `notify-debouncer-full`，loopback 写路径 + reflink 摄取；
- 设备归属：节点自身 `device_id`（常开"家"的 files 空间对所有 owner 可读）；
- 场景：NAS 文件夹（相机导入、共享盘）自动进 store 并被索引；
- 验收：拖文件进绑定目录 → versions / index / search 全链路；进程重启后对账一致；
  跨卷自动降级 copy。

## 11. 验收清单

### 自动化

- 摄取三模式（reflink/hardlink/copy）单元测试 + 降级链测试；
- 对账 diff：新增 / 修改 / 删除 / rename（move 识别与退化路径）；
- 忽略规则、半写保护、事件丢失后定期对账收敛；
- hardlink 就地修改 → 新 inode 重新摄取 + 旧版本 warning。

### 手工（并入 QA_BASELINE.md）

- Phase A/B 各自的端到端验收（§9 / §10）；
- 性能：10k 文件目录首扫 < 30s（本地基准）；增量 diff < 1s。

## 12. 里程碑建议

| 阶段 | 内容 | 预估 |
|------|------|------|
| M6a | App 绑定目录（copy 先行）+ UI + 对账 | 1-2 周 |
| M6b | 节点 `--bind` + reflink/hardlink + ingest op | 2-3 周 |

依赖：M0-M5 已就绪（commit 钩子、版本、索引）；`ingest` 为增量 op，无破坏性变更。

## 13. 风险与对策

| 风险 | 对策 |
|------|------|
| reflink 不可用（ext4/NTFS/跨卷） | 自动降级 copy，文档明示占用翻倍 |
| hardlink 就地修改污染旧版本 | 仅不可变目录启用；inode+mtime 检测重新摄取 |
| inotify 溢出 / 事件丢失 | 定期全量对账兜底（正确性不依赖事件） |
| 大目录资源占用 | 扫描限速；单绑定默认上限 50 万文件（可配） |
| rename 误判 | sha256 + inode 双信号；误判退化为 delete+add 并审计 |
