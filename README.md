# Nexuspouch

无头 store master 节点：实现 `store.*` 协议语义与 ACL，可与 [ShePaw](https://github.com/shepaw/shepaw) App（Flutter `lib/storage`）配对互通。

本仓库由 ShePaw 的 `storage-node` 拆出独立维护。

## 范围

- 路径规范化 + ACL；本机目录树 `list/meta/read/write/commit/delete/stats/recycle/import.*`
- `commit.retention`（`keep_last` / `gfs`）
- HTTP `/health`、`/store`（联调 JSON）
- **Noise IK 配对**：`/peer/ws`（信封 v2 + `Noise_IK_25519_ChaChaPoly_BLAKE2b`，prologue `shepaw-acp/2.1`）；device_id = Noise fingerprint
- **无头管理面** `/admin`：配对 QR/批准、用量、回收站、换机导入审批；token 或 loopback 鉴权

协议权威说明见 [`docs/storage_protocol_spec.md`](docs/storage_protocol_spec.md)；共享 fixture 在 [`docs/storage_fixtures/`](docs/storage_fixtures/)。

## 与 ShePaw 的互操作

为保持与现有 App 扫码配对兼容，线协议仍使用：

- QR：`shepaw://peer?...`
- Noise prologue：`shepaw-acp/2.1`

管理鉴权环境变量优先读 `NEXUSPOUCH_*`，并兼容旧的 `SHEPAW_*`。

## 运行

```bash
go test ./...
go run ./cmd/nexuspouch \
  -root /var/lib/nexuspouch \
  -listen :8787 \
  -name "nas-master" \
  -admin-token "$NEXUSPOUCH_ADMIN_TOKEN"
```

1. 打开 `http://127.0.0.1:8787/admin/`，点「开始配对」得到 `shepaw://peer?...` QR。
2. ShePaw App 扫码发起配对；节点 `/admin` 出现入站请求后批准。
3. 配对成功后 App 可经 Noise 加密 WS 发送 store 控制帧。

身份文件：`<root>/.system/noise_identity.json`；配对表：`paired_peers.json`。

## 模块

```text
github.com/zoujunrong/Nexuspouch
```
