# Windows 服务化安装（W2）

> 目标：闲置 Windows PC 开机自启 Nexuspouch，日志落盘，便于当 master。

## 便携运行（无服务）

```powershell
# 解压 release zip 后
$env:NEXUSPOUCH_ROOT = "D:\nexuspouch-data"
.\nexuspouch.exe serve --listen 0.0.0.0:18787
```

日志：默认 stderr；生产建议重定向到文件或使用下方 NSSM。

## NSSM（推荐）

1. 安装 [NSSM](https://nssm.cc/)（或 `choco install nssm`）。
2. 安装服务：

```powershell
nssm install Nexuspouch "C:\Program Files\Nexuspouch\nexuspouch.exe" serve --listen 0.0.0.0:18787
nssm set Nexuspouch AppDirectory "C:\Program Files\Nexuspouch"
nssm set Nexuspouch AppEnvironmentExtra NEXUSPOUCH_ROOT=D:\nexuspouch-data
nssm set Nexuspouch AppStdout D:\nexuspouch-data\logs\stdout.log
nssm set Nexuspouch AppStderr D:\nexuspouch-data\logs\stderr.log
nssm set Nexuspouch Start SERVICE_AUTO_START
nssm start Nexuspouch
```

## sc.exe（无 NSSM）

```powershell
sc.exe create Nexuspouch binPath= "\"C:\Program Files\Nexuspouch\nexuspouch.exe\" serve --listen 0.0.0.0:18787" start= auto
sc.exe description Nexuspouch "Nexuspouch store node"
# 环境变量需用系统属性或包装脚本设置 NEXUSPOUCH_ROOT
sc.exe start Nexuspouch
```

## 防火墙

首次配对前放行入站 TCP（默认 `18787`）与 mDNS（UDP 5353，若用局域网发现）：

```powershell
New-NetFirewallRule -DisplayName "Nexuspouch" -Direction Inbound -Protocol TCP -LocalPort 18787 -Action Allow
```

## 验收清单

- [ ] 重启后服务自动起来
- [ ] 手机 / 另一台 PC 可配对
- [ ] admin UI / store 读写正常
- [ ] 磁盘将满时 `volume_*` 字段有值（W2 `GetDiskFreeSpaceEx`）
