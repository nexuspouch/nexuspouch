# Windows 安装与便携包（W3）

> 配套 [SERVICE.md](SERVICE.md)。目标：闲置 Windows PC 10 分钟内可当 master。

## 1. 便携 zip（推荐先试）

1. 从 GitHub Releases / CI artifact 下载 `nexuspouch-windows-x64.zip`；
2. 解压到如 `D:\Nexuspouch\`；
3. PowerShell：

```powershell
$env:NEXUSPOUCH_ROOT = "D:\nexuspouch-data"
New-Item -ItemType Directory -Force -Path $env:NEXUSPOUCH_ROOT | Out-Null
.\nexuspouch.exe serve --listen 0.0.0.0:18787 --name home-pc
```

4. 浏览器打开 `http://127.0.0.1:18787/admin`，用启动日志中的 admin token 登录；
5. 配对手机 / 另一台 PC。

## 2. 一键服务安装（NSSM）

见 [SERVICE.md](SERVICE.md)。装好后开机自启，日志落在 `$NEXUSPOUCH_ROOT\logs\`。

## 3. 长路径

- Store 相对路径本身不受 `MAX_PATH` 限制；外部绑定目录若超过 ~260 字符，建议：
  - 开启系统长路径：`Computer\HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled = 1`，或
  - 把绑定根目录放到较短路径（如 `D:\bind\docs`）；
- Nexuspouch **拒绝** `\\?\` 扩展路径写入 store 相对路径（安全）；绑定扫描使用 OS 原生路径 API。

## 4. 防火墙与 mDNS

```powershell
New-NetFirewallRule -DisplayName "Nexuspouch" -Direction Inbound -Protocol TCP -LocalPort 18787 -Action Allow
```

局域网发现依赖 DNS-SD / mDNS（UDP 5353）。公司网 / 访客 Wi‑Fi 常屏蔽组播——此时用 admin「Channel」或手动填节点 URL。

## 5. 验收清单（代码侧已具备；真机勾选）

- [ ] `cargo test --lib` 在 Windows CI 绿（workflow：`ci.yml`）
- [ ] 服务开机自启 + admin 可开
- [ ] 手机扫码配对
- [ ] 绑定目录增删改同步
- [ ] `stats` 出现 `volume_*` 字段
