# Nexuspouch Admin UI

独立前端（Vite + TypeScript），当前包含 **会话管理**（`/admin/sessions`）。

Rust 节点在检测到 `web/admin/dist/sessions/index.html` 时托管静态资源；否则回退到内嵌 HTML。

## 开发

终端 A — 节点：

```bash
cargo run -- --root ./data --listen 127.0.0.1:8787 --no-mdns
```

终端 B — 前端 dev server（API 代理到 8787）：

```bash
cd web/admin
npm install
npm run dev
```

浏览器打开：**http://127.0.0.1:5173/index.html**（Vite dev；API 代理到 8787）

Hash 路由：`#/`, `#/search`, `#/bind`, `#/s/<uri>`

## 生产构建

```bash
cd web/admin
npm ci
npm run build
```

产物：`web/admin/dist/sessions/`。重启 `nexuspouch` 后访问 http://127.0.0.1:8787/admin/sessions/

也可显式指定：

```bash
NEXUSPOUCH_ADMIN_STATIC=/path/to/web/admin/dist cargo run -- ...
```

## 目录

```
web/admin/
  sessions/index.html   # 入口
  src/shared/           # api、dom、format
  src/sessions/         # 会话 SPA（pages + router）
  dist/sessions/        # build 输出（gitignore）
```

主管理面 `/admin` 仍为 Rust 内嵌 HTML；后续可迁入 `src/main/`。
