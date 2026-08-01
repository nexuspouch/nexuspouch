# Nexuspouch

Rust headless storage node for the ShePaw ecosystem. Implements `store.*` over Noise-encrypted WebSocket peer connections, with a local admin UI for pairing and management.

Wire format is compatible with the ShePaw Flutter app (`Noise_IK_25519_ChaChaPoly_BLAKE2b`, `shepaw://peer` QR URLs, store control frames).

## Requirements

- Rust 1.70+ (edition 2021)
- macOS / Linux (Unix filesystem semantics)

## Build

```bash
cargo test
cargo build --release
```

The release binary is `target/release/nexuspouch`.

Install (systemd / Docker): see [docs/INSTALL.md](docs/INSTALL.md).

## Run

```bash
cargo run -- --root ./data --listen :8787 --name nexuspouch
```

### Subcommands

```bash
# Connectivity JSON (local / LAN browse / Channel DNS·TCP·WS)
cargo run -- doctor --listen :8787 --channel wss://relay.example/peer

# One-shot encrypted reprotect (ShePaw-compatible pack)
cargo run -- reprotect --root ./data --password '…'
```

### CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--root` | `./data` | Store root directory |
| `--listen` | `:8787` | HTTP listen address (`:8787` → `0.0.0.0:8787`) |
| `--name` | `nexuspouch` | Device display name for pairing |
| `--admin-token` | env | Admin API/UI token (`NEXUSPOUCH_ADMIN_TOKEN` or `SHEPAW_ADMIN_TOKEN`) |
| `--channel` | env | Channel WS endpoint for QR (`NEXUSPOUCH_CHANNEL_ENDPOINT` or `SHEPAW_CHANNEL_ENDPOINT`) |
| `--device` | Noise fp | Override device_id (must match Noise fingerprint) |
| `--no-mdns` | off | Skip mDNS service registration (browse still works) |

When `--admin-token` is unset, `/admin` and `/admin/api/*` accept requests without a Bearer token (use only on trusted networks).

## mDNS & diagnostics

LAN discovery uses DNS-SD service type **`_nexuspouch._tcp.local.`** with TXT records:

| Key | Value |
|-----|-------|
| `fp` | Noise device fingerprint |
| `name` | Device display name |
| `path` | `/peer/ws` |
| `proto` | Protocol version (`4`) |

Admin API (Bearer token required when configured):

| Path | Description |
|------|-------------|
| `GET /admin/api/discovery` | Browse LAN peers via mDNS (~1.5s) |
| `GET /admin/api/diagnostics` | Local health, LAN browse, channel TCP reachability |

`GET /health` includes `"mdns": true/false` when the node is advertising.

## HTTP endpoints

| Path | Description |
|------|-------------|
| `GET /health` | Liveness + device fingerprint |
| `POST /store` | Loopback store debug API (token or loopback client) |
| `GET /peer/ws` | Noise pairing, reconnect, encrypted store frames |
| `GET /admin` | Admin UI (pairing, stats, recycle, import) |
| `/admin/api/*` | Admin JSON API |
| `/api/v1/*` | Programmable store HTTP API (see [docs/API.md](docs/API.md)) |
| `/dav/*` | Read-only WebDAV for `artifacts` and `files` |

## HTTP API & WebDAV

Token-authenticated programmable API at `/api/v1` plus read-only WebDAV at `/dav`. See [docs/API.md](docs/API.md) for URI format, curl examples, and SSE events.

When `--admin-token` is unset, `/api/v1` and `/dav` accept loopback clients only (same as `/admin`).

## Identity & storage layout

- Noise identity: `<root>/.system/noise_identity.json` (STANDARD base64 keys)
- `device_id` = first 8 bytes of `sha256(static_pub)` as 16 lowercase hex
- Master pointer: `<root>/.system/master_pointer.json` (created on first open)
- Paired peers: `<root>/.system/paired_peers.json`

## Protocol

See [docs/storage_protocol_spec.md](docs/storage_protocol_spec.md) and shared fixtures in [docs/storage_fixtures/](docs/storage_fixtures/).

ShePaw App integration (pairing, mDNS, reprotect): [docs/INTEGRATION.md](docs/INTEGRATION.md).

## Implemented

- `master.migrate` with online seed from previous master, mirror hash gate, and outbound dial via `Dialer`
- `commit.retention` policies (`keep_last`, `gfs`) after successful promote
- `stats.volume_*` and `volume_warn` (Unix `statvfs`; omitted on non-Unix)
- Mirror reprotect: ShePaw-compatible `manifest.json` + `mirror.tar.enc` (password via admin POST or `NEXUSPOUCH_REPROTECT_PASSWORD`)
- Full-text search (`/api/v1/search`, SQLite FTS5) with auto-indexing and optional summary hook

## Agent 接入

Nexuspouch 面向 AI agent 协作：agent 拿到 `store://` URI 即可读写、校验、追溯产物（本地优先、数据不出硬件）。

- **现在**：HTTP API + Bearer token（[docs/API.md](docs/API.md)）+ 只读 WebDAV + MCP 服务器（`nexuspouch mcp`，stdin/stdout）
- **接入指南**：[docs/AGENTS.md](docs/AGENTS.md)（身份/作用域/配额/引用纪律）
- **路线**：M2 版本化 URI 与血缘；M3 交接/事件；M4 agent 身份与配额；M5 检索（[docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md)）

## Development

```bash
# Run fixture + noise tests only
cargo test protocol:: noise::

# Run with logging
RUST_LOG=info cargo run -- --root ./data
```
