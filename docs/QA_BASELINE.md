# ShePaw ↔ Nexuspouch 集成基线 QA（M0 遗留项）

> 目标：把「App ↔ Nexuspouch 现有全链路」固化为可重复的验收基线，
> 覆盖 M2-M5 新增能力。自动化的部分已由双端测试守住；手工项在真机/桌面执行。

## 0. 自动化基线（每次改动跑一遍）

```bash
# Nexuspouch（Rust）：63 测试
cd /Users/edenzou/workspace/Nexuspouch && cargo test

# ShePaw（Dart）协议层：15 测试（含 version_cases / handoff_cases 契约）
cd /Users/edenzou/workspace/shepaw/shepaw && flutter test test/storage/store_protocol_test.dart

# 双端 gate（任一侧 fixture 修改后）
SHEPAW_REPO=/Users/edenzou/workspace/shepaw/shepaw \
  /Users/edenzou/workspace/Nexuspouch/scripts/check_fixtures.sh
```

> 已知既有 flake：`snapshot_service_test` 恢复计数断言与
> `scheduled_snapshot_test` 密码变更测试在基线同样失败（时序相关，与本次协议改动无关）。

## 1. 手工基线：配对 → 同步 → 浏览 → 回收站 → 管理页

前置：NAS/桌面跑 `nexuspouch --root ./data --listen :8787 --name home-nas`
（或 Docker / systemd），App 与节点同一局域网。

- [ ] 节点 admin 页显示配对 QR；App 扫描批准后出现在「已配对设备」
- [ ] mDNS：App 存储页能发现节点；节点重启换 IP 后已配对连接可刷新 endpoint
- [ ] App 写 artifacts（Agent 产物 / 附件）→ master 镜像目录可见
- [ ] 双设备 owner：A 端产物经 master 在 B 端可读（`store.list` / 缩略图 / 下载）
- [ ] master 离线：本机数据完整可用；跨端读降级（stale 缓存或报缺）
- [ ] 删除文件进 `.recycle`；回收站列表可见、可还原；清空仅 master 本机
- [ ] 卷用量 ≥80% 触发 `volume_warn` 告警（可在小卷/容器上构造）
- [ ] admin：stats / browse / devices purge / wipe-self / GC / audit 均可用

## 2. M2 版本化与血缘

- [ ] 同一路径连续 commit 3 次 → `/api/v1/versions?uri=` 返回 3 个版本
- [ ] `read@v1` / `read@v2` 取回对应内容；`@<sha16>` 前缀解析
- [ ] commit 带 `manifest`（producer / parent_uris）→ `/api/v1/manifest` 可见血缘
- [ ] 外部尝试读写 `.versions` / `.nexuspouch` → `bad_path`
- [ ] publish 产物多次覆盖 → 版本不被修剪；非 publish 按 keep_last 修剪进回收站

## 3. M3 交接与事件

- [ ] `handoff.create`（带 context）→ 事件 `handoff.created`；消费方
      `handoff.ack` → 状态 `acked`；重复 ack 幂等
- [ ] 覆盖后 `artifact.state?uri=...@v1` 显示 `superseded`
- [ ] watcher 断开后 `GET /api/v1/events?since=<seq>` 重放不丢不重
- [ ] App 经 master 读取产物后自动 ack（`acks.json` 出现记录）

## 4. M4 Agent 身份与配额

- [ ] admin 创建 agent（scopes + max_bytes）→ 绑定 token → 作用域外操作
      403 `acl_denied`；超配额 `quota_exceeded`
- [ ] 吊销 agent 后立即拒绝；审计可按 agent 过滤
- [ ] `nexuspouch mcp-agent --agent <id>` 以该身份运行且受限

## 5. M5 检索

- [ ] 写入含关键词的 md → `/api/v1/search?q=` 命中并返回 snippet
- [ ] delete 后索引移除；`nexuspouch index rebuild` 重建一致
- [ ] 配置 `NEXUSPOUCH_SUMMARY_URL` 后 commit 异步生成摘要并进入索引

## 6. 验收判定

- 全部自动化基线绿 + 手工项无阻塞缺陷 → 基线通过。
- 任一新增协议行为（versions/handoff/search）与双端 fixture 不一致 → 阻断发布。
