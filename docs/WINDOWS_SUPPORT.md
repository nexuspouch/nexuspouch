# Windows 节点支持设计（Windows Support）

> 状态：W1 进行中（2026-08-02，分支 `codex/windows-w1`）
> 决策：**支持 Windows 作为 Nexuspouch 节点的一等目标**——用闲置 PC
> （主流用户为 Windows）当 master 是常见场景。
> 范围：仅 Nexuspouch（Rust）节点服务；ShePaw App 桌面端（Flutter）与
> channel（Go）本就跨平台。
>
> W1 已做：路径盘符/UNC/`\\?\`/`//server` 拒绝（双端 fixture）；
> Windows 上 `reflink_copy` 直接降级；hardlink 测试去 Unix-only 断言；
> `libc` 仅 unix 依赖；CI matrix 含 `windows-latest`。

## 1. 现状与缺口

| 方面 | Unix 现状 | Windows 缺口 |
|------|-----------|-------------|
| 路径语义 | `/`；拒绝盘符/UNC | store 相对路径与 FS 绝对路径需分离核对；盘符/UNC 判定语义不同 |
| 符号链接纪律 | `is_symlink` | 识别 reparse point（junction/symlink）；`mklink` 需权限 |
| 原子 rename | staging→dest 覆盖 | rename 覆盖已存在文件行为不同；文件占用（编辑器/杀毒）会锁 |
| reflink | `cp -c` / `--reflink=auto` | NTFS 无 CoW → 绑定摄取直接降级 copy（已有降级路径，确认并文档化） |
| 文件事件 | FSEvents / inotify | ReadDirectoryChangesW（notify 已支持，需验证 + 长短路径） |
| mDNS/防火墙 | Bonjour/multicast | DNS-SD 兼容 + 防火墙入站放行指引 |
| 服务化 | systemd / Docker | Windows 服务（NSSM / sc.exe）+ 安装文档 |
| 卷统计 | `statvfs`（volume.rs） | `GetDiskFreeSpaceEx` |
| 路径长度 | 无 | MAX_PATH 260（长路径 `\\?\`） |
| CI/测试 | cargo test（macOS/Linux） | 加 Windows runner；fixture 双端测试在 Windows 绿 |

## 2. 里程碑

### W1 基础可用

| 项 | 状态 |
|----|------|
| 路径语义（盘符 / UNC / `\\?\` / `//server` / 根相对 `\`） | ✅ fixture + 单元测试；Dart 对齐 |
| Windows 编译障碍（`cp --reflink`、unix MetadataExt 测试、libc） | ✅ |
| CI `windows-latest` + `cargo test --lib` | ✅ workflow 已加（待 Actions 跑绿确认） |
| 配对 / mDNS / store / admin / WebDAV / MCP 真机 | 待 Windows runner 绿后抽测 |

### W2 完整特性

- 符号链接纪律适配：reparse point 识别与拒绝；
- 原子 rename 兼容 + 文件占用重试；
- 卷统计：`volume.rs` 增加 Windows 实现（`GetDiskFreeSpaceEx`）；
- 文件事件验证：notify（ReadDirectoryChangesW）在绑定目录摄取上跑通；
- reflink：确认 NTFS 自动降级 copy，文档写明；
- 服务化：Windows 服务安装脚本（NSSM/sc.exe）+ 日志落盘 + 开机自启文档。

### W3 打磨

- 安装包/一键安装指引；防火墙放行指引；
- 长路径处理（如需）；mDNS 在 Windows 网络环境实测；
- 真机/Windows 实测条目并入 DEVICE_TESTING.md。

## 3. 测试与验收

- `cargo test` 在 Windows runner 全绿（与 macOS/Linux 同基线）；
- fixture 双端（Rust + Dart）在 Windows 上行为一致；
- 绑定目录摄取：新增/修改/删除/忽略/降级路径全链路；
- 验收：闲置 Windows PC 装服务 → 手机/另一台 PC 配对 → 同步/版本/检索/绑定可用。

## 4. 风险与对策

| 风险 | 对策 |
|------|------|
| 路径/权限语义差异 | W1 先做路径核对 + CI 全绿再扩展 |
| 文件锁导致 rename 失败 | 重试 + 文档提示（关闭占用程序/杀毒白名单） |
| 平台维护成本 | 特性分层（W1 基础 → W2 完整 → W3 打磨），CI 自动守住 |
| 体积/安装 | 提供 zip 便携版 + 服务安装脚本两档 |
