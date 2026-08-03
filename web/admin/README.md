# Nexuspouch Admin UI

独立前端（**React 19 + Vite + TypeScript**），视觉 token 对齐 agent-bridge
（Catppuccin Mocha）。当前页面：会话管理 `/admin/sessions`。

Rust 检测到 `web/admin/dist/sessions/index.html` 时托管静态资源；否则回退内嵌 HTML。

## 开发

```bash
# 终端 A — 节点
cargo run -- --root ./data --listen 127.0.0.1:8787 --no-mdns

# 终端 B — 前端
cd web/admin
npm install
npm run dev
```

打开 http://127.0.0.1:5173/index.html （API 代理到 8787）

Hash：`#/` 总览（左右分栏）· `#/search` · `#/bind` · `#/s/<uri>`

## 生产构建

```bash
cd web/admin && npm ci && npm run build
```

产物：`web/admin/dist/sessions/`。重启 nexuspouch 后访问
http://127.0.0.1:8787/admin/sessions/

```bash
NEXUSPOUCH_ADMIN_STATIC=/path/to/web/admin/dist cargo run -- ...
```

## 目录

```
web/admin/
  sessions/index.html
  src/shared/           # api / format
  src/sessions/         # App + components + styles
  dist/sessions/        # build 输出（gitignore）
```

主管理面 `/admin` 仍为 Rust 内嵌 HTML，后续可迁入 React。
