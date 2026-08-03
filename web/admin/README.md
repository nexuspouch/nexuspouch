# Nexuspouch Admin UI

独立前端（**React 19 + Vite + TypeScript**），视觉 token 对齐 agent-bridge
（Catppuccin Mocha）。

| 页面 | URL | 入口 |
|------|-----|------|
| 主管理面 | `/admin/` | `main/` |
| 会话管理 | `/admin/sessions/` | `sessions/` |

Rust 检测到对应 `web/admin/dist/*/index.html` 时托管静态资源；否则回退内嵌 HTML。

## 开发

```bash
# 终端 A — 节点
cargo run -- --root ./data --listen 127.0.0.1:8787 --no-mdns

# 终端 B — 主管理面
cd web/admin && npm install && npm run dev
# → http://127.0.0.1:5173/ （API 代理到 8787）

# 或会话页
npm run dev:sessions
# → http://127.0.0.1:5174/
```

会话 Hash：`#/` 总览 · `#/search` · `#/bind` · `#/s/<uri>`

## 生产构建

```bash
cd web/admin && npm ci && npm run build
```

产物：`web/admin/dist/main/`、`web/admin/dist/sessions/`。重启 nexuspouch 后访问：

- http://127.0.0.1:8787/admin/
- http://127.0.0.1:8787/admin/sessions/

```bash
NEXUSPOUCH_ADMIN_STATIC=/path/to/web/admin/dist cargo run -- ...
```

## 目录

```
web/admin/
  main/index.html
  sessions/index.html
  vite.main.config.ts
  vite.sessions.config.ts
  src/shared/           # api / format / theme
  src/main/             # 主管理面 panels
  src/sessions/         # 会话管理
  dist/main|sessions/   # build 输出（gitignore）
```
