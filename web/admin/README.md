# Nexuspouch Admin UI

独立前端（**React 19 + Vite + TypeScript**），视觉 token 对齐 agent-bridge
（Catppuccin Mocha）。**一个 Vite 工程、一次启动**，两个 HTML 入口：

| 页面 | URL |
|------|-----|
| 主管理面 | `/admin/` |
| 会话管理 | `/admin/sessions/` |

Rust 检测到 `web/admin/dist` 对应 HTML 时托管；否则回退内嵌 HTML。

## 开发

```bash
# 终端 A — 节点
cargo run -- --root ./data --listen 127.0.0.1:8787 --no-mdns

# 终端 B — 前端（同时服务两个页面）
cd web/admin && npm install && npm run dev
```

- 主管理面：http://127.0.0.1:5173/admin/
- 会话管理：http://127.0.0.1:5173/admin/sessions/

会话 Hash：`#/` 总览 · `#/search` · `#/bind` · `#/s/<uri>`

## 生产构建

```bash
cd web/admin && npm ci && npm run build
```

产物：`web/admin/dist/`（`index.html` + `sessions/` + 共享 `assets/`）。
重启 nexuspouch 后访问节点上的 `/admin/` 与 `/admin/sessions/`。

```bash
NEXUSPOUCH_ADMIN_STATIC=/path/to/web/admin/dist cargo run -- ...
```

## 目录

```
web/admin/
  index.html            # 主管理面
  sessions/index.html   # 会话管理
  vite.config.ts        # 统一入口
  src/shared/           # api / format / theme
  src/main/             # 主管理面
  src/sessions/         # 会话管理
  dist/                 # build 输出（gitignore）
```
