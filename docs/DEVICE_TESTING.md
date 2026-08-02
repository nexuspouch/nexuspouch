# 真机集成测试路径（Device Testing Path）

> 目的：在真实设备上端到端验证 ShePaw App ↔ Nexuspouch 节点全链路
> （配对/同步/版本/交接/agent 配额/检索/目录绑定/迁移）。
> 对照 [QA_BASELINE.md](QA_BASELINE.md) 的验收项；本文件给出可执行的操作顺序。

## 0. 环境准备

### 节点（Nexuspouch）

```bash
cargo build --release
./target/release/nexuspouch --root ./data --listen :8787 --name home-nas
# 另开终端确认：
curl -s http://127.0.0.1:8787/health   # 记下 device（16 hex）
```

### App（ShePaw）

- 真机 A（手机）与真机 B（手机）安装同一构建：`flutter build ios --release` /
  `flutter build apk --release`（debug 也可，速度更快）；
- 两台手机与节点同一局域网；iOS 需在 Info.plist 配置
  `NSLocalNetworkUsageDescription` 与 `NSBonjourServices`（`_nexuspouch._tcp`）；
- 桌面端可选（macOS）作为第三台设备参与交接测试。

### 辅助工具

- 笔记本 curl（验证 API）/ admin UI：`http://<节点IP>:8787/admin`；
- 三台设备各记下自己的 `device_id`（App 存储页 / 节点 peers 列表）。

## 1. 配对与发现

1. 节点 admin →「开始配对」→ 显示 QR；
2. 手机 A 扫 QR 配对（owner）；手机 B 同样配对；
3. 预期：admin「已配对设备」出现两个 device_id；
4. mDNS：App 存储页「NAS 入口」能发现节点；断 Wi-Fi 重连后 endpoint 自动刷新；
5. 验证：手机 A 存储页显示自己的 `<device_id>` 目录存在。

## 2. 写路径与镜像（本地优先）

1. 手机 A 通过任意 Agent/附件写入 `artifacts`；
2. 预期：本机文件树立即出现；节点 admin「browse」或
   `curl "$NODE/api/v1/list?uri=store://artifacts/<A>/"` 能看到镜像；
3. **master 离线**：停节点 → 手机 A 继续写（本地优先）→ 重启节点 → 未同步队列自动补齐
   （admin stats `unsynced_count` 归零）。

## 3. M2 版本与血缘

```bash
# 对同一路径连续 commit 3 次（可用 MCP store_write 或 POST /api/v1/store）
curl -s "$NODE/api/v1/versions?uri=store://artifacts/<A>/task-1/out.txt"
# 预期：3 个版本（v1/v2/v3），protected=false
curl -s "$NODE/api/v1/read?uri=store://artifacts/<A>/task-1/out.txt@v1"
# 预期：返回第一次写入的内容
```

- commit 时带 `"manifest"`（producer/parent_uris）→
  `curl "$NODE/api/v1/manifest?uri=store://artifacts/<A>/task-1/"` 显示血缘；
- App 端（若版本 UI 已实现）：浏览任务目录可看版本列表并打开旧版本。

## 4. M3 交接与事件

1. 手机 A 用 `handoff.create` 发布产物（带 `context`）；
2. 预期：`GET /api/v1/events/recent` 出现 `handoff.created`；
3. 手机 B 读取该产物（App 打开 store:// URI）→ 自动 ack；
4. 预期：`artifact.state?uri=...` 显示 `acked`、`acked_by=<B>`；
5. 覆盖同路径 → `artifact.state?uri=...@v1` 显示 `superseded`；
6. 断线重放：`GET /api/v1/events?since=<seq>` 不丢不重。

## 5. M4 Agent 身份与配额

1. admin「Agents」创建只读 agent → 绑定 token；
2. 用该 token 调 `POST /api/v1/store` 写 → 预期 403 `acl_denied`；
3. 创建小配额 agent → 超限写 → `quota_exceeded`；
4. admin 吊销 agent → 立即拒绝；`/admin/api/audit` 可按 agent 过滤。

## 6. M5 检索

1. 手机 A 写入含关键词的 `report.md`；
2. `curl -G --data-urlencode "q=关键词" "$NODE/api/v1/search"` → 命中 + snippet；
3. 删除该文件 → 再次搜索无结果；
4. `nexuspouch index rebuild` 后结果一致（兜底）。

## 7. M6 目录绑定

### 节点侧（Phase B）

```bash
./target/release/nexuspouch --root ./data --listen :8787 \
  --bind "/Users/me/Downloads=files/downloads:auto"
# 拖文件进 Downloads → watcher 触发摄取（≤1s + 500ms 去抖）
curl -s "$NODE/api/v1/list?uri=store://files/<节点device>/downloads/"
```

- `hardlink-immutable` 模式：`ls -i` 对比外部文件与 store 文件 inode 一致（不占双份）；
- 删除外部文件 → store 对应文件进回收站。

### App 侧（Phase A）

- 存储管理页 →「目录绑定」→ 填路径/文件夹名 → 添加（自动首次同步）；
- 改目录内文件 → 手动「同步」或轮询周期 → 手机 B 可见且有版本。

## 8. 迁移与再保护

1. admin「master/migrate」把 master 从节点切到手机 B：
   - 预期：种子差量拉取 + `hash_gate` 摘要；各端改指；
2. admin「再保护」输入主密码 → `.system/reprotect/<ts>/` 生成
   `manifest.json + mirror.tar.enc`（App 侧可用同一主密码解密验证）。

## 9. Windows 节点（W3 勾选）

对照 [windows/INSTALL.md](windows/INSTALL.md) / [windows/SERVICE.md](windows/SERVICE.md)：

1. 便携 zip 或 NSSM 服务启动；admin 可开；
2. 防火墙放行后手机扫码配对；
3. 绑定一本地目录 → 增/改/删/改名（sha）→ store 同步；
4. `GET /admin/api/stats` 含 `volume_total_bytes` / `volume_free_bytes`；
5. （可选）断网重连后 mDNS 或手动 URL 仍可连。

## 10. 通过标准

- 第 1-8 步全部达到预期，无阻塞缺陷；Windows 节点另勾第 9 步；
- 任一协议行为与双端 fixture 不一致 → 阻断（先回协议评审）。

## 常见问题

| 现象 | 排查 |
|------|------|
| 手机扫不到节点 | 同一局域网？iOS 本地网络权限？节点 `--listen` 是否 0.0.0.0 |
| 配对成功但不同步 | admin peers 是否 owner；master 指针是否指向节点 |
| 搜索不到刚写的文件 | 索引随 commit 异步写入；等 1s 重试或 `index rebuild` |
| 绑定目录没反应 | watcher 日志（`RUST_LOG=info`）；确认外部目录权限可读 |
| Windows 配对失败 | 防火墙 TCP 端口；是否监听 `0.0.0.0`；见 windows/SERVICE.md |
