# ClipRoam

ClipRoam 是一个本地优先的跨平台剪贴板历史与设备漫游工具。桌面端使用 Tauri 2、Vue 3 和 TypeScript；同步服务使用 TypeScript、Fastify、WebSocket 和 SQLite。

## 当前可运行链路

- 自动采集文本剪贴板并保存到应用数据目录
- Windows 自动采集资源管理器复制的文件列表，支持一次复制多个文件或整个文件夹（含空目录），并可从历史中再次粘贴还原原有目录结构
- 应用正常启动显示主界面；`Ctrl + Shift + V`（macOS 为 `Cmd + Shift + V`）打开精简的快速粘贴列表
- 搜索、键盘选择、固定、删除与清理历史
- Windows 选择条目后自动写入剪贴板并粘贴到原应用
- 多账号同步服务，账号之间的剪贴板历史和在线设备完全隔离
- 密码使用加盐哈希保存，客户端仅保存可过期的登录会话
- 服务不可用时自动降级为纯本地模式
- 文件按内容寻址（`fileId = sha256(内容)`）存储：相同内容只保存一份，服务器已持有的内容直接秒传，改名或换路径后再复制也不会重传
- 文件可按每台客户端设置的阈值自动上传；上传中断后会从服务器保留的分片位置自动续传；未上传的大文件在源设备在线时通过 WebSocket 中继

复制文件后内容标识在后台计算，条目会立刻出现在历史里并标注「计算中」，此时已可在本机粘贴；标识齐全后才发布到服务器。

剪贴板条目只引用内容标识，目录结构单独保存在条目的树里，因此目录本身不占存储、同一内容出现在多个路径也只保存一份。本机复制再本机粘贴时直接使用原始路径，不产生任何副本；原文件已移动或删除时才在缓存目录中用硬链接重建视图。远端设备粘贴文件时会先下载缺失内容到自己的应用缓存目录，再写入系统文件剪贴板。服务器已有副本时不要求源设备在线；只有未自动上传的文件才要求源设备在线。

## 服务端结构

- `apps/server/src/index.ts`：进程入口，只启动服务。
- `app/`：`ClipRoamServer`、运行配置与 WebSocket 连接类型。
- `services/`：`AuthService`（账号和限流）与 `FileTransferService`（上传、续传、下载中继）。
- `storage/`：`AccountStore`、`UserDataStore`、`ClipRoamStore` 与数据路径。

## 开发

```powershell
pnpm install
pnpm dev:server
pnpm dev
```

桌面端首次启动会要求填写服务器 `IP:端口`、连接协议、账号和密码，可直接登录或注册。登录后只在当前设备保存 30 天会话令牌，不保存密码；服务端按“账号 + 设备”保留一个会话，同一设备重新登录会替换旧令牌，多个设备可同时登录。也可以暂时仅使用本地剪贴板，之后从窗口顶部的连接状态重新配置。默认开发地址为 `127.0.0.1:4810`，可通过桌面端的 `VITE_CLIPROAM_SERVER` 覆盖。

服务器默认将所有持久化数据放在 `$HOME/.cliproam`。账号与会话位于 `$HOME/.cliproam/accounts.sqlite`；每个用户位于 `$HOME/.cliproam/users/<userId>/`，其中 `data.sqlite` 保存该用户剪贴板记录和内容索引，`files/<内容标识前两位>/<内容标识>` 保存实际文件。内容池按用户隔离，不跨账号去重。Docker 部署时挂载该目录即可保留全部服务端数据，例如 `-v cliproam-data:/root/.cliproam`；也可单独备份或恢复某个用户目录。可用 `CLIPROAM_DATA_DIRECTORY` 修改数据根目录，`CLIPROAM_ACCOUNTS_DATABASE`、`CLIPROAM_USERS_DIRECTORY` 分别覆盖账号库与用户目录。不再被任何剪贴板记录引用的内容由后台回收，删除记录累计到一定次数、以及每 6 小时会各触发一次。`CLIPROAM_MAX_STORED_FILE_MB` 控制服务器允许持久化的单文件上限，默认 100MB；`CLIPROAM_UPLOAD_RESUME_TTL_HOURS` 控制未完成上传分片的保留时长，默认 24 小时。每台客户端可在连接设置中独立选择更小的自动上传阈值。

## HTTPS 与管理后台

直接运行服务端源码（包括 VS Code 的 `Launch Server`）时，管理后台默认密码为 `admin`，仅供本地开发。编译后的服务必须设置非空的管理员密码，再访问 `http(s)://服务器地址:端口/admin`：

```powershell
$env:CLIPROAM_ADMIN_PASSWORD = "请替换为高强度管理员密码"
pnpm --filter @cliproam/server start
```

`CLIPROAM_PORT`、`CLIPROAM_ADMIN_PASSWORD`、`CLIPROAM_MAX_STORED_FILE_MB` 和 `CLIPROAM_UPLOAD_RESUME_TTL_HOURS` 可以由启动环境传入；后两个值在后台保存后会由后台配置覆盖。

从 `src/` 启动的开发模式（VS Code 的 `Launch Server`、`pnpm dev:server`）默认管理员密码为 `admin`；编译后的正常 `start` 不提供默认密码。

管理后台可上传 PEM 格式的完整证书链和私钥，并支持替换或删除后台托管的证书。证书保存在 `$HOME/.cliproam/tls/`；服务已经使用 HTTPS 时会热加载替换后的证书，HTTP 服务首次配置证书后必须重启，下一次启动会自动以 HTTPS/WSS 监听同一端口。删除证书后也必须重启，重启后同一端口将回到 HTTP/WS。也可不使用后台，改用 `CLIPROAM_TLS_CERT_FILE` 与 `CLIPROAM_TLS_KEY_FILE` 指向证书和私钥文件；两者必须同时配置，且此模式下后台只读。请仅在受信任的网络中通过 HTTP 初始配置证书；正式环境应直接使用 HTTPS 或在可信 TLS 反向代理后访问后台。

管理后台也可调整服务器文件上限和断点续传有效期，配置保存在 `$HOME/.cliproam/server-settings.json`，优先于启动时的 `CLIPROAM_MAX_STORED_FILE_MB` 与 `CLIPROAM_UPLOAD_RESUME_TTL_HOURS` 默认值。将任一值设为 `0` 分别表示禁止服务器存储文件或禁用断点续传。

## 验证

```powershell
pnpm check
pnpm build
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
```
