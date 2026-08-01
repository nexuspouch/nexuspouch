# ShePaw ↔ Nexuspouch 集成指南

Nexuspouch 是常开的本地 store master；ShePaw App 通过 Noise 配对后把本机空间镜像到节点，并可用局域网发现自动刷新 endpoint。

## 线协议兼容（勿改）

| 项 | 值 |
|----|-----|
| Noise | `Noise_IK_25519_ChaChaPoly_BLAKE2b` |
| Prologue | `shepaw-acp/2.1` |
| QR | `shepaw://peer?code=…&pk=…&fp=…&local=…` |
| device_id | `sha256(static_pub)` 前 8 字节 → 16 hex |
| Store frames | `ns=store` / `v=4` 控制帧 |

## 首次配对

1. 启动节点：`nexuspouch --root ./data --listen :8787 --name home-nas`
2. Admin → 开始配对 → 显示 QR（或扫屏幕上的 `shepaw://peer…`）
3. App 扫码批准 → peer 入库，App 可 `setMasterDeviceId` 指向节点

首次仍需 QR（含一次性 `code` / 静态公钥）。mDNS **不能**替代配对，只负责发现与刷新 LAN 地址。

## 局域网发现

### 节点侧

- 广播 DNS-SD：`_nexuspouch._tcp.local.`
- TXT：`fp` / `name` / `path=/peer/ws` / `proto=4`
- 关闭广播：`--no-mdns`

### App 侧（ShePaw）

- `NexuspouchDiscoveryService` browse `_nexuspouch._tcp`
- 储物袋 → NAS 入口：列表发现结果
  - **未配对**：引导扫码配对
  - **已配对**（fp 命中）：`updateLocalEndpoint` + `connectToPeer` + `setMasterDeviceId`

平台注意：iOS/macOS 需 `NSBonjourServices` 含 `_nexuspouch._tcp`。

## 可编程访问（非 App）

见 [API.md](API.md)：

- `store://` URI、`/api/v1/*`、SSE 事件
- 只读 WebDAV `/dav`
- 鉴权：scoped Bearer token（或 loopback）

Agent / 脚本读产物优先走 HTTP API；移动 App 与信任链仍走 `/peer/ws`。

## 再保护（mirror reprotect）

与 ShePaw `MirrorReprotectService` 对齐：

```
<master>/backups/reprotect-YYYYMMDD-HHMMSS/
  manifest.json      # kind=mirror_reprotect
  mirror.tar.enc     # XChaCha20-Poly1305(nonce‖ct‖tag)
```

KDF：

```
H   = PBKDF2-HMAC-SHA256(password, "shepaw.storage.v1", 120000)
key = HMAC-SHA256(H, "shepaw.snapshot.key" ‖ salt)
```

触发：

- Admin UI「再保护」并输入与 App 相同的主密码
- 或 `POST /admin/api/reprotect` + `{"password":"..."}`
- 或环境变量 `NEXUSPOUCH_REPROTECT_PASSWORD`
- 无密码时退化为清单模式（`mode=manifest`，无 `mirror.tar.enc`）
- 调试明文树：`NEXUSPOUCH_REPROTECT_COPY=1`

保留：`keep_last=4`，仅 `reprotect-*` 前缀。

## 推荐拓扑

```
[ShePaw phone] ──LAN/QR──► [Nexuspouch NAS] ◄──LAN── [ShePaw desktop]
                              master
                         artifacts/files/…
```

- 手机/桌面保持 owner 配对；master 指针指向 Nexuspouch
- Channel（可选）仅作穿越 endpoint 配置，节点不托管中继

## 验收清单

- [ ] `cargo test` / App `flutter test test/storage/nexuspouch_discovery_txt_test.dart`
- [ ] 扫码配对后 App 能 `store.list` / 写 artifacts
- [ ] mDNS 改 IP 后，已配对设备 Connect 能刷新 local endpoint
- [ ] 加密 reprotect：节点生成的 `mirror.tar.enc` 可用同一主密码在 App 侧解密（或反过来）
