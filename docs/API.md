# Nexuspouch HTTP API

Programmable store access over HTTP (Bearer token). Noise WebSocket (`/peer/ws`) remains the path for App pairing and encrypted peer RPC.

## Store URI

ShePaw-aligned format:

```
store://<space>/<device_id>/<relative/path>
```

| Field | Values |
|-------|--------|
| `space` | `artifacts`, `files`, `attachments`, `backups` |
| `device_id` | 16 lowercase hex chars (Noise fingerprint) |
| `path` | Normalized relative path (no `..`, no leading `/`) |

Examples:

- `store://artifacts/aaaaaaaaaaaaaaaa/task-41/output.png`
- `store://files/aaaaaaaaaaaaaaaa/docs/readme.md`

Path-style also accepted: `store:///artifacts/aaaaaaaaaaaaaaaa/task-41/output.png`

## Authentication

- Header: `Authorization: Bearer <token>` (same token as `--admin-token` / `NEXUSPOUCH_ADMIN_TOKEN`)
- SSE: `?token=<token>` query param (for browser `EventSource`)
- If no token is configured, only loopback clients (`127.0.0.1` / `::1`) are allowed

## Endpoints

Base: `/api/v1`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness + device + mDNS status |
| GET | `/uri/resolve?uri=` | Parse URI and return file/dir metadata |
| GET | `/read?uri=&offset=&length=` | Read file bytes (octet-stream) |
| GET | `/list?uri=` | List directory entries |
| GET | `/list?space=&device=&path=` | List without URI |
| POST | `/store` | Store frame API (`{"op","payload"}`) |
| GET | `/events` | SSE stream (`event: snapshot` then `event: store`) |
| GET | `/events/recent` | Last N events as JSON |

## curl examples

```bash
TOKEN=your-admin-token
BASE=http://127.0.0.1:8787
URI='store://artifacts/aaaaaaaaaaaaaaaa/task-1/hello.txt'

# Health
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/api/v1/health" | jq

# Resolve metadata
curl -s -G -H "Authorization: Bearer $TOKEN" \
  --data-urlencode "uri=$URI" \
  "$BASE/api/v1/uri/resolve" | jq

# Read file
curl -s -G -H "Authorization: Bearer $TOKEN" \
  --data-urlencode "uri=$URI" \
  "$BASE/api/v1/read"

# List directory
curl -s -G -H "Authorization: Bearer $TOKEN" \
  --data-urlencode "uri=store://files/aaaaaaaaaaaaaaaa/" \
  "$BASE/api/v1/list" | jq

# Recent store events
curl -s -H "Authorization: Bearer $TOKEN" \
  "$BASE/api/v1/events/recent" | jq

# SSE (use ?token= for EventSource)
curl -N -H "Accept: text/event-stream" \
  "$BASE/api/v1/events?token=$TOKEN"

# Store op (same as POST /store, but works off-loopback with token)
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"op":"stats","payload":{}}' \
  "$BASE/api/v1/store" | jq
```

## WebDAV (read-only)

Mount: `/dav/{space}/{device}/{path...}`

Supported spaces: `artifacts`, `files` only (`attachments` / `backups` → 403).

Methods: `OPTIONS`, `GET`, `HEAD`, `PROPFIND` (depth 0/1). Writes return `405`.

```bash
curl -s -X PROPFIND -H "Authorization: Bearer $TOKEN" \
  -H "Depth: 1" \
  "$BASE/dav/files/aaaaaaaaaaaaaaaa/"
```

## Rust SDK helper

`nexuspouch::sdk::Client` builds URLs and wraps `ureq` calls:

```rust
let c = nexuspouch::sdk::Client::new("http://127.0.0.1:8787", "token");
let meta = c.resolve("store://files/aaaaaaaaaaaaaaaa/a.txt")?;
let bytes = c.read_text("store://files/aaaaaaaaaaaaaaaa/a.txt")?;
```

## Noise peer path

For mobile App pairing, encrypted sync, and friend-trust ACL, continue using:

- `GET /peer/ws` — Noise IK handshake + store frames
- Admin UI `/admin` — QR pairing

The HTTP API is for automation, scripts, and read-only WebDAV mounts on trusted networks.
