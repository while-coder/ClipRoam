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

复制文件后先进入待上传列表并标注「计算中」，内容标识齐全后逐项发布到服务器。历史记录由服务器分页查询，与待上传列表独立维护。

剪贴板条目只引用内容标识，目录结构单独保存在条目的树里，因此目录本身不占存储、同一内容出现在多个路径也只保存一份。本机复制再本机粘贴时直接使用原始路径，不产生任何副本；原文件已移动或删除时才在缓存目录中用硬链接重建视图。Windows 远端粘贴会立即向资源管理器提供虚拟文件，普通文件边下载边读取，小文件包完成校验后再展开；macOS/Linux 会先下载缺失内容，再把恢复后的真实路径写入 Finder 或文件管理器的剪贴板。服务器已有副本时不要求源设备在线；只有未自动上传的文件才要求源设备在线。

macOS 首次自动粘贴时需要在“系统设置 → 隐私与安全性 → 辅助功能”中允许 ClipRoam。Linux 的文件剪贴板同时支持 X11 与 Wayland；自动模拟 `Ctrl+V` 在 X11 需要 `xdotool`，Wayland 优先使用 `wtype`，也可使用 `ydotool`。

## 客户端系统与目录

| 系统 | 前端 `apps/app/src/features/` | Rust `apps/app/src-tauri/src/` |
| --- | --- | --- |
| 历史记录 | `history/`：分页查询、补查记录和文件详情、复制/粘贴入口 | `history/`：记录缓存读取、更新、删除及剪贴板写入 |
| 待上传列表 | `pending-upload/`：队列展示、逐项发布、自动上传文件及进度 | `pending_upload/`：监听剪贴板、捕获入队、哈希解析、出队 |
| 下载列表 | `downloads/`：异步下载调用和任务快照 | `downloads/`：全局 FIFO 队列、逐项下载、取消、另存 |
| 上传列表 | `uploads/`：其他设备请求触发的并发中继发送与任务展示 | 共用 `file/read.rs` 读取本机内容 |

历史按 `manifest → 缺失记录补查 → 缺失文件状态补查 → 本地摘要 → UI` 查询，不读取待上传队列。剪贴板捕获只更新待上传列表，成功发布后才由服务器历史查询显示记录。

下载通过异步 `downloadFiles()` 等待 Rust 队列结果；同一时刻下载一个文件，所有窗口共用队列。上传列表收到 `file.requested` 即启动发送，各中继会话并发执行，不等待前一个任务完成，以免服务器等待超时。

公共基础设施保留独立目录：前端 `sync/` 管理会话、HTTP 和推送；Rust `content/`、`file/`、`store/`、`platforms/` 管理内容结构、缓存、持久化和系统适配。服务端保持现有目录和接口，`clipboard/` 管理用户历史，`files/` 管理内容存储与中继，`app/routes/` 提供查询、发布和文件传输接口。管理后台 `apps/admin` 与协议包 `packages/protocol` 保持独立。

## 开发

```powershell
pnpm install
pnpm --filter @cliproam/server dev
pnpm --filter @cliproam/app tauri dev
```

构建服务端 Docker 镜像并导出到项目根目录的 `cliproam-server.tar`：

```powershell
pnpm docker
```

命令会从根 `package.json` 读取版本号，同时生成版本标签与 `latest` 标签；导出的 tar 保留版本标签。

### Android / iOS

移动端复用登录、设备、历史、搜索、固定、删除和前台同步。Android/iOS 不启动桌面托盘、全局快捷键和后台剪贴板轮询；点按文本条目会复制到系统剪贴板。Android 点按图片或文件条目会先弹出系统目录选择框，再下载并保存到所选目录；下载中再次点按同一条目可取消，取消选择目录不会开始下载。iOS 暂时仍下载到应用缓存。Android 可从系统分享面板接收文字、图片和文件：文字与单张图片按一次本机复制进入历史和同步链路，文件及多张图片只进入历史；iOS 系统分享、文件导出和后台传输仍需各自的原生扩展。

Android 首次生成工程并构建 APK：

```powershell
pnpm --filter @cliproam/app android:init
pnpm --filter @cliproam/app android:build
```

Android 页面调试（无需独立 Web 版本）：

1. 启动模拟器，或在真机开启 USB 调试；运行 `adb devices` 确认设备状态为 `device`。MuMu 需要在模拟器设置中开启 ADB，再使用 `adb connect 127.0.0.1:<模拟器显示的 ADB 端口>` 连接。
2. VS Code 选择 `Launch App · Android` 并按 F5，或运行 `pnpm --filter @cliproam/app android:dev`。终端中按提示选择设备；Tauri 会安装并启动开发版，Vite 提供前端热更新。
3. 电脑 Chrome 打开 `chrome://inspect/#devices`，启用 `Discover USB devices`，在 ClipRoam 的 WebView 下点击 `inspect`，即可检查 DOM/CSS、Console、Network 和 JavaScript 断点。F5 入口负责启动 Android 开发进程，页面断点在 Chrome DevTools 中调试。

Vite 会读取 Tauri 设置的 `TAURI_DEV_HOST`，设备需要能访问电脑的开发地址和 `1430` 端口。页面运行在 Android WebView 内，具有真实的 Tauri 接口和移动端平台能力；仅用桌面浏览器的窄屏模式不会切换到移动端逻辑。

iOS 必须在安装了 Xcode 的 macOS 上执行：

```bash
pnpm --filter @cliproam/app ios:init
pnpm --filter @cliproam/app ios:build
```

Android debug 构建允许连接开发用 HTTP 服务；release 和 iOS 正式包应连接 HTTPS/WSS 服务。

桌面端首次启动会要求填写服务器 `IP:端口`、连接协议、账号和密码，可直接登录或注册。登录后只在当前设备保存 30 天会话令牌，不保存密码；服务端按“账号 + 设备”保留一个会话，同一设备重新登录会替换旧令牌，多个设备可同时登录。也可以暂时仅使用本地剪贴板，之后从窗口顶部的连接状态重新配置。默认开发地址为 `127.0.0.1:4810`。

服务器默认将所有持久化数据放在 `$HOME/.cliproam`。账号与会话位于 `accounts.sqlite`；每个用户的剪贴板记录和设备信息位于 `users/<userId>/data.sqlite`。全局内容池的索引和续传状态位于 `files.sqlite`，实际文件位于 `files/<内容标识前两位>/<内容标识>`。文件按内容哈希跨账号去重，共享断点续传状态；相同内容已存储时直接返回已完成，无需重复上传。服务信任客户端声明的文件哈希，下载时检查文件是否被当前账号自己的历史条目引用。Docker 部署时挂载数据目录，并显式设置管理后台密码：

```powershell
docker run -d --name cliproam-server -p 4810:4810 -e CLIPROAM_ADMIN_PASSWORD="请替换为高强度管理员密码" -v cliproam-data:/root/.cliproam cliproam-server:latest
```

容器内部固定监听 `4810`，如需使用其他宿主机端口，只需修改 `-p` 左侧端口，例如 `-p 8080:4810`。备份应包含完整数据目录，用户历史与全局文件池关联。服务器不自动回收文件；删除历史或账号后，文件仍占用磁盘，需在管理后台手动删除。单文件上限默认 200MB（上传文件须小于该值），未完成上传默认支持 24 小时续传，到期后再次上传该文件会重新开始。历史条数和单次复制文件数由管理后台配置，发布接口也会校验文件数量和目录深度。每台客户端可在连接设置中选择更小的自动上传阈值。

## HTTPS 与管理后台

Docker 和 `pnpm --filter @cliproam/server dev` 均不提供默认管理员密码，也不会将密码写入日志。必须设置非空的管理员密码，才能登录 `http(s)://服务器地址:端口/admin`：

```powershell
$env:CLIPROAM_ADMIN_PASSWORD = "请替换为高强度管理员密码"
pnpm --filter @cliproam/server start
```

`CLIPROAM_ADMIN_PASSWORD` 可以由启动环境传入。服务固定监听 `4810`；需要更换对外端口时，请通过 Docker 端口映射或反向代理完成。

VS Code 的 `Launch Server` 从启动环境继承 `CLIPROAM_ADMIN_PASSWORD`；请在启动 VS Code 前设置该环境变量。

管理后台可上传 PEM 格式的完整证书链和私钥，并支持替换或删除后台托管的证书。证书保存在 `$HOME/.cliproam/tls/`；服务已经使用 HTTPS 时会热加载替换后的证书，HTTP 服务首次配置证书后必须重启，下一次启动会自动以 HTTPS/WSS 监听同一端口。删除证书后也必须重启，重启后同一端口将回到 HTTP/WS。请仅在受信任的网络中通过 HTTP 初始配置证书；正式环境应直接使用 HTTPS 或在可信 TLS 反向代理后访问后台。

新证书与私钥保存在同一个原子替换的 `tls/pair.json` 文件中，兼容已有的 `cert.pem` / `key.pem`。已配置的证书损坏或缺失时，服务启动失败，不会自动降级为 HTTP。

管理后台也可调整服务器文件上限和断点续传有效期，配置保存在 `$HOME/.cliproam/server-settings.json`。将任一值设为 `0` 分别表示禁止服务器存储文件或禁用断点续传。

## 验证

```powershell
pnpm check
pnpm build:server
pnpm --filter @cliproam/app build
cargo check --manifest-path apps/app/src-tauri/Cargo.toml
```
