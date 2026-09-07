# ClipRoam

ClipRoam 是一个本地优先的跨平台剪贴板历史与设备漫游工具。桌面端使用 Tauri 2、Vue 3 和 TypeScript；同步服务使用 TypeScript、Fastify、WebSocket 和 `better-sqlite3`。

## 当前可运行链路

- 自动采集文本剪贴板并保存到应用数据目录
- Windows、macOS 与 Linux 自动采集文件列表和图片，支持一次复制多个文件或整个文件夹（含空目录），并可从历史中再次粘贴还原原有目录结构
- 应用正常启动显示主界面；`Ctrl + Shift + V`（macOS 为 `Cmd + Shift + V`）打开精简的快速粘贴列表
- 搜索、键盘选择、固定、删除与清理历史
- 同账号在线设备会实时接收剪贴板并写入本机系统剪贴板，可按设备关闭；桌面端支持文本、富文本和图片，移动端当前仅自动接收文本；文件与文件夹只同步到历史，接收设备不会为其自动创建本地缓存或覆盖系统剪贴板
- 选择条目后自动写入系统剪贴板并粘贴到原应用；Windows 远端文件按需流式读取，macOS/Linux 会先将缺失内容完整恢复到本地缓存视图
- 多账号同步服务，账号之间的剪贴板历史和在线设备完全隔离
- 密码使用加盐哈希保存，客户端仅保存可过期的登录会话
- 服务不可用时自动降级为纯本地模式
- 文件按内容寻址（`fileId = sha256(内容)`）存储：相同内容只保存一份，服务器已持有的内容直接秒传，改名或换路径后再复制也不会重传
- 文件可按每台客户端设置的阈值自动上传；上传中断后会从服务器保留的分片位置自动续传；未上传的大文件在源设备在线时通过 WebSocket 中继

复制文件后内容标识在后台计算，条目会立刻出现在历史里并标注「计算中」，此时已可在本机粘贴；标识齐全后才发布到服务器。

剪贴板条目只引用内容标识，目录结构单独保存在条目的树里，因此目录本身不占存储、同一内容出现在多个路径也只保存一份。本机复制再本机粘贴时直接使用原始路径，不产生任何副本；原文件已移动或删除时才在缓存目录中用硬链接重建视图。Windows 远端粘贴会立即向资源管理器提供虚拟文件，普通文件边下载边读取，小文件包完成校验后再展开；macOS/Linux 会先下载缺失内容，再把恢复后的真实路径写入 Finder 或文件管理器的剪贴板。服务器已有副本时不要求源设备在线；只有未自动上传的文件才要求源设备在线。

macOS 首次自动粘贴时需要在“系统设置 → 隐私与安全性 → 辅助功能”中允许 ClipRoam。Linux 的文件剪贴板同时支持 X11 与 Wayland；自动模拟 `Ctrl+V` 在 X11 需要 `xdotool`，Wayland 优先使用 `wtype`，也可使用 `ydotool`。

## 服务端结构

- `apps/server/src/index.ts`：进程入口，只启动服务。
- `app/`：`ClipRoamServer`、运行配置与 WebSocket 连接类型。
- `files/`：内容寻址的文件子系统，与剪贴板记录相互独立。`FileStore` 是按用户隔离的内容池（落盘路径、内容索引、回收），`UploadService`（秒传、分块账本续传、内容校验）与 `FileDownloadService`（下载需求登记）各管一半生命周期。文件字节与下载编排全部走 HTTP：`PUT /upload/:fileId` 上传、`GET /files/:entryId/:fileId` 下载（内容缺失时登记需求并立即返回 `NOT_STORED`，客户端重试）、`GET /files/requests` 供持有内容的设备长轮询领取需求并推送；WebSocket 只负责剪贴板同步。这一层不认识条目，只认识内容标识。
- `services/`：`AuthService`（账号和限流）、`AdminService` 与 `TlsCertificateService`。
- `storage/`：`AccountStore`、`UserDataStore`（剪贴板记录与设备）、`ClipRoamStore` 与数据路径。

## 开发

```powershell
pnpm install
pnpm dev:server
pnpm dev
```

构建服务端 Docker 镜像并导出到项目根目录的 `cliproam-server.tar`：

```powershell
pnpm docker
```

命令会从根 `package.json` 读取版本号，同时生成版本标签与 `latest` 标签；导出的 tar 保留版本标签。

### Android / iOS

移动端复用登录、设备、历史、搜索、固定、删除和前台同步。Android/iOS 不启动桌面托盘、全局快捷键和后台剪贴板轮询；点按文本条目会复制到系统剪贴板，点按图片或文件条目会把缺失内容下载到应用缓存。Android 可从系统分享面板接收文字、图片和文件：文字与单张图片按一次本机复制进入历史和同步链路，文件及多张图片只进入历史；iOS 系统分享、文件导出和后台传输仍需各自的原生扩展。

Android 首次生成工程并构建 APK：

```powershell
pnpm --filter @cliproam/app android:init
pnpm --filter @cliproam/app android:build
```

iOS 必须在安装了 Xcode 的 macOS 上执行：

```bash
pnpm --filter @cliproam/app ios:init
pnpm --filter @cliproam/app ios:build
```

Android debug 构建允许连接开发用 HTTP 服务；release 和 iOS 正式包应连接 HTTPS/WSS 服务。

桌面端首次启动会要求填写服务器 `IP:端口`、连接协议、账号和密码，可直接登录或注册。登录后只在当前设备保存 30 天会话令牌，不保存密码；服务端按“账号 + 设备”保留一个会话，同一设备重新登录会替换旧令牌，多个设备可同时登录。也可以暂时仅使用本地剪贴板，之后从窗口顶部的连接状态重新配置。默认开发地址为 `127.0.0.1:4810`。

服务器默认将所有持久化数据放在 `$HOME/.cliproam`。账号与会话位于 `$HOME/.cliproam/accounts.sqlite`；每个用户位于 `$HOME/.cliproam/users/<userId>/`，其中 `data.sqlite` 保存该用户剪贴板记录和内容索引，`files/<内容标识前两位>/<内容标识>` 保存实际文件。内容池按用户隔离，不跨账号去重。Docker 部署时挂载该目录即可保留全部服务端数据，并通过 `CLIPROAM_ADMIN_PASSWORD` 设置管理后台密码：

```powershell
docker run -d --name cliproam-server -p 4810:4810 -e CLIPROAM_ADMIN_PASSWORD="请替换为高强度管理员密码" -v cliproam-data:/root/.cliproam cliproam-server:latest
```

容器内部固定监听 `4810`，如需使用其他宿主机端口，只需修改 `-p` 左侧端口，例如 `-p 8080:4810`。也可单独备份或恢复某个用户目录。不再被任何剪贴板记录引用的内容由后台回收，删除记录累计到一定次数、以及每 6 小时会各触发一次。服务器单文件上限默认 100MB，未完成上传分片默认保留 24 小时；这两项可在管理后台调整。每台客户端可在连接设置中独立选择更小的自动上传阈值。

## HTTPS 与管理后台

直接运行服务端源码（包括 VS Code 的 `Launch Server`）时，管理后台默认密码为 `admin`，仅供本地开发。编译后的服务必须设置非空的管理员密码，再访问 `http(s)://服务器地址:端口/admin`：

```powershell
$env:CLIPROAM_ADMIN_PASSWORD = "请替换为高强度管理员密码"
pnpm --filter @cliproam/server start
```

`CLIPROAM_ADMIN_PASSWORD` 可以由启动环境传入。服务固定监听 `4810`；需要更换对外端口时，请通过 Docker 端口映射或反向代理完成。

从 `src/` 启动的开发模式（VS Code 的 `Launch Server`、`pnpm dev:server`）默认管理员密码为 `admin`；编译后的正常 `start` 不提供默认密码。

管理后台可上传 PEM 格式的完整证书链和私钥，并支持替换或删除后台托管的证书。证书保存在 `$HOME/.cliproam/tls/`；服务已经使用 HTTPS 时会热加载替换后的证书，HTTP 服务首次配置证书后必须重启，下一次启动会自动以 HTTPS/WSS 监听同一端口。删除证书后也必须重启，重启后同一端口将回到 HTTP/WS。请仅在受信任的网络中通过 HTTP 初始配置证书；正式环境应直接使用 HTTPS 或在可信 TLS 反向代理后访问后台。

管理后台也可调整服务器文件上限和断点续传有效期，配置保存在 `$HOME/.cliproam/server-settings.json`。将任一值设为 `0` 分别表示禁止服务器存储文件或禁用断点续传。

## 验证

```powershell
pnpm check
pnpm build
cargo check --manifest-path apps/app/src-tauri/Cargo.toml
```
