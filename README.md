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

## Run

```bash
cargo run -- --root ./data --listen :8787 --name nexuspouch
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

When `--admin-token` is unset, `/admin` and `/admin/api/*` accept requests without a Bearer token (use only on trusted networks).

## HTTP endpoints

| Path | Description |
|------|-------------|
| `GET /health` | Liveness + device fingerprint |
| `POST /store` | Loopback store debug API (token or loopback client) |
| `GET /peer/ws` | Noise pairing, reconnect, encrypted store frames |
| `GET /admin` | Admin UI (pairing, stats, recycle, import) |
| `/admin/api/*` | Admin JSON API |

## Identity & storage layout

- Noise identity: `<root>/.system/noise_identity.json` (STANDARD base64 keys)
- `device_id` = first 8 bytes of `sha256(static_pub)` as 16 lowercase hex
- Master pointer: `<root>/.system/master_pointer.json` (created on first open)
- Paired peers: `<root>/.system/paired_peers.json`

## Protocol

See [docs/storage_protocol_spec.md](docs/storage_protocol_spec.md) and shared fixtures in [docs/storage_fixtures/](docs/storage_fixtures/).

## Implemented

- `master.migrate` with online seed from previous master, mirror hash gate, and outbound dial via `Dialer`
- `commit.retention` policies (`keep_last`, `gfs`) after successful promote
- `stats.volume_*` and `volume_warn` (Unix `statvfs`; omitted on non-Unix)

## Development

```bash
# Run fixture + noise tests only
cargo test protocol:: noise::

# Run with logging
RUST_LOG=info cargo run -- --root ./data
```
