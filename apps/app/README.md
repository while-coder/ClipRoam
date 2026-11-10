# ClipRoam客户端

Vue + TypeScript 实现界面和业务逻辑，Tauri 提供原生能力。主窗口与快速粘贴共用一个原生窗口，由 TS 切换标题栏、尺寸、位置和置顶状态。

## 代码边界

- `../../packages/ui/src`：三端共用的登录和业务页面、账号状态、历史缓存、同步、捕获过滤、哈希、上传与下载队列，以及平台接口和共用 Web API 实现。
- `src`：Tauri 入口壳、`NativeRust`、原生日志和更新器接入、快捷粘贴窗口、独立 Toast 窗口及其样式。快速粘贴视图通过共享 App 插槽接入；Toast 窗口单独加载，不初始化共享账号与同步业务。
- `src-tauri/src`：原生命令与平台实现。Rust 提供系统剪贴板、图片格式适配、焦点与粘贴按键、文件字节读写、系统对话框、Android 文件导出、窗口与托盘；系统分享由 Kotlin / Swift 接收并保存，打开主 App 后导入共用 TS 待上传队列。
- `src-tauri/native`：Android、iOS、macOS 的原生源码、配置、资源与准备脚本。CI 分别执行 `android:init`、`ios:init` 生成工程，`src-tauri/gen` 不纳入 Git；详情见[原生目录说明](src-tauri/native/README.md)。

入口先注入 `NativeRust`，再加载共用 App；浏览器开发预览使用共用 Web API 适配器。文件导出、账号数据库、同步和传输调度由共享 TS 代码负责。

## 应用名称与语言

`productName` 固定为 `ClipRoam`，安装包、Release 标题和技术标识沿用英文名称，bundle ID 仍为 `com.whilecode.cliproam`。界面、窗口标题、通知和托盘中的应用名称跟随系统语言（浏览器中跟随浏览器语言）：中文显示名从本地化资源读取，其他语言回退为 ClipRoam。本次仅本地化应用名称，其他界面文案仍为中文。

iOS 使用 `src-tauri/native/ios/locales/*.lproj/InfoPlist.strings`，macOS 使用 `src-tauri/native/macos/locales/*.lproj/InfoPlist.strings`，分别维护系统显示名，Android 使用 `src-tauri/native/android/app/src/main/res/values*/strings.xml`，Linux 桌面入口使用 `Name`/`Name[zh]`。Windows 安装器、开始菜单和快捷方式名称保留 ClipRoam，运行时窗口及托盘名称按语言选择；不使用自定义 NSIS 模板。

## 本地数据

“剪贴板漫游”按内容类型分别控制：监听剪贴板包含文字（含富文本）、截图（含图片）、文件（含文件夹）三个开关；自动接收剪贴板包含文字和截图两个开关，文件仍需从历史手动选择粘贴。保存后生效，旧版总开关的状态会继承到对应的各项；关闭监听不影响手动导入。

桌面端支持全局老板键，默认 `Alt+Shift+H`（macOS 为 `Option+Shift+H`）：窗口可见且未最小化时隐藏，再次按下恢复主界面。可在“设置 → 快捷键”关闭或录制新组合，保存后生效；配置仅保存在当前设备。老板键由原生端处理，不依赖后台 WebView 回调；新组合注册失败时保留旧组合，且不能与快捷粘贴共用组合。

主界面登录配置包含服务器地址及协议、用户名和 sessionToken，使用 AES-GCM 加密后保存到 IndexedDB 的 `cliproam-global/meta`，每次保存使用新的随机 IV。随机密钥保存在同一数据库，目的是避免直接保存明文；这不提供系统钥匙串级别的隔离。退出登录删除当前登录配置，配置或密钥损坏后可重新登录覆盖。

移动端分享入口只保存文本和附件，然后打开主 App。登录和初始化完成后导入当前账号的待上传列表；分享入口不额外保存登录凭据。iOS 使用 App Group 在扩展与主应用之间共享收件箱。详见[移动端分享说明](src-tauri/share/README.md)。

历史缓存、待上传队列、账号偏好、设备标识和下载记录使用 IndexedDB，账号数据按服务器与用户名隔离。桌面文件内容保存在应用目录，浏览器文件内容保存为 IndexedDB Blob。没有旧 SQLite、旧配置或钥匙串数据导入流程；升级后需要重新登录，历史从服务器重新获取。

## 验证与浏览器边界

在仓库根目录执行 `pnpm --filter @cliproam/app build`；在 `src-tauri` 执行 `cargo check --offline`。

业务核心可供 PWA 或浏览器插件复用，当前尚未加入 PWA manifest、Service Worker 或插件打包。浏览器不提供全局快捷键、后台系统剪贴板监听、向其他应用投递粘贴按键和系统文件剪贴板；读取或写入剪贴板仍受浏览器权限约束。

浏览器支持目录选择 API 时，多文件和文件夹可保存到用户选择的目录，保留嵌套目录和空目录；每个文件下载并校验后流式写入。不支持该 API 时只提供单文件下载，导出文件夹或多文件会明确提示不支持。取消或失败时停止当前文件写入，已保存的文件保留在所选目录。

## iOS 27 短信自动同步快速添加

iOS 16 及更新版本提供“同步文本到 ClipRoam”“同步文件到 ClipRoam”两个快捷指令动作。多文件和文件夹保留目录结构及空目录。图片可通过文件动作作为普通文件上传，保留原始文件。上传成功会更新同账号其他在线客户端的历史列表，不会自动覆盖系统剪贴板。

两个动作共用 Keychain 内的上传专用凭证；凭证不能读取历史、下载或删除记录。退出登录、重新登录或会话失效后，需要由 App 重新配置。旧的文字上传凭证会在服务端升级时自动迁移。

快捷指令动作随 App 安装提供；iOS 27 的快速添加入口还需要发布一份带“收到信息”触发器的预制自动化。在已安装当前 App 的 iOS 27 设备上创建并验证：信息正文作为“同步文本到 ClipRoam”的文本参数，不加入账号、服务器地址或上传令牌，然后通过系统分享取得 iCloud 链接。

将真实分享链接写入 `apps/app/.env.production.local` 的 `VITE_IOS_SMS_SHORTCUT_URL` 后重新构建 iOS App。只接受 `https://www.icloud.com/shortcuts/` 下的 32 位快捷指令 ID 链接。开发调试可在 `.env.development.local` 配置同名变量。分享链接是公开模板，不应包含任何用户数据或凭据。

配置有效链接后，iOS 27 及更新版本的设置页可打开系统添加页面；用户仍须确认添加、开启自动化、关闭运行前确认并完成首次授权。iOS 26 保留手动指引；尚未配置真实模板链接时，快速添加按钮禁用并提供手动配置说明。打开链接不代表用户已添加或启用自动化，App 不显示安装成功状态。

### 上传接口

iOS 和 Android 使用相同的原生上传配置内容：`uploadUrl`（完整的 `/shortcuts/text` URL）、`uploadToken`（独立上传凭证）、`expiresAt`（服务端返回的 ISO 8601 有效期）、`sessionFingerprint`（登录凭证的 SHA-256，用于判断是否需要重新配置）。登录后保存，退出登录后清除。iOS 存入 Keychain，Android 存入应用私有且排除备份的文件；短信触发方式和 Android 待上传队列独立于这份配置。iOS 已保存的旧版服务器地址会在读取时转换为完整上传 URL。

使用普通登录凭证调用 `POST /shortcuts/token` 获取上传凭证，之后在下列请求中携带 `Authorization: Bearer <upload token>`：

| 接口 | 请求格式 | 内容 |
| --- | --- | --- |
| `POST /shortcuts/text` | `application/json` | `{ "text": "内容" }` |
| `POST /shortcuts/files` | `multipart/form-data` | 一个或多个 `file` / `files` 文件字段，生成一条文件记录 |

文件字段的 `filename` 可填写相对路径（如 `资料/说明.txt`）。可增加一个 `directories` 文本字段，值为 JSON 字符串数组（如 `["资料/空目录"]`），保留空目录。重复文件路径、绝对路径和 `..` 路径会被拒绝。所有文件接收并校验完成后才发布记录，限制沿用账号的文件大小、文件数量和历史条数配置。

两个接口成功时均返回 `{ "entry": { "id": "..." } }`，并向同账号在线客户端发送 `clipboard.created`。`DELETE /shortcuts/token` 可撤销当前设备的上传凭证；上传凭证也可调用这个撤销接口。

### iOS 便签小组件

在主屏幕添加「随心贴 便签」（英文环境为「ClipRoam Notes」）小组件后，显示当前登录账号最近的便签。小、中、大尺寸分别显示最近 1、3、5 条；图片和文件显示名称，不下载附件。

小组件通过现有 `GET /entries/sticky` 接口独立联网，申请每 30 分钟更新一次；实际刷新时间由 iOS 调度，不保证准时。App 收到记录变化后也会请求刷新。网络失败时保留上次成功的缓存和更新时间；登录失效时清空缓存并提示打开 App。

登录配置与缓存保存在受文件保护、排除备份的 App Group `StickyWidget/state.json` 中，供主应用和小组件共享。切换账号或退出登录会清除旧账号的快照；首次使用、重新登录或服务器地址变更后需打开一次 App。小组件复用当前登录凭证，不使用仅供上传的快捷指令凭证。

`ios:init`、`ios:dev` 和 `ios:build` 会通过 `native/ios/prepare.cjs` 生成并嵌入 WidgetKit 扩展。SideStore 重签名时须保留 `ClipRoamWidget.appex`，并授予它与主应用相同的 App Group。Windows 可运行 `node --test apps/app/src-tauri/native/ios/prepare.test.cjs` 验证工程生成；Swift 编译、真机添加小组件及后台刷新需在 macOS/Xcode 和 iOS 设备验证。

## Android 新短信同步

Android 登录成功（包括恢复登录）后，原生插件会申请独立上传凭证，并将完整的 `/shortcuts/text` 上传 URL、凭证及有效期保存在应用私有的 `noBackupFilesDir/sms-session.json` 中，不保存完整登录凭证。首次配置会请求“接收短信”权限；拒绝后可在系统应用权限中允许，或重新登录重试。

取得权限后，Manifest 中的原生短信接收器会合并分段短信，将所有新收到的短信正文同步到同账号的剪贴板历史，不读取历史收件箱，也不筛选验证码。接收后先持久化待上传记录，再立即发起短超时 HTTP 请求；失败时由 WorkManager 在网络可用后重试。待上传短信保存在同一私有、不参与备份的目录中，超过 10 分钟后丢弃。退出登录或切换账号会清理旧凭证和待上传短信，并尝试撤销服务端上传凭证，避免串号上传。

同步不依赖 WebView 常驻。强行停止应用、撤销短信权限、厂商后台限制及网络异常仍会影响接收或上传；真机锁屏、进程回收、断网恢复和多 SIM 卡短信需实机验证。此入口同步的是系统交付的 SMS 广播，不读取其他应用通知或 RCS 消息。桌面端沿用现有历史更新链路。

## macOS 签名与辅助功能授权

macOS 默认使用固定自签名证书 `cliproam-dev` 签名整个 `.app`。Bundle ID 保持 `com.whilecode.cliproam`；持续使用同一张证书，使签名要求不再绑定每次构建变化的 `cdhash`。缺少证书时构建失败，不自动降级为 ad-hoc 签名。自签名证书不提供 Apple 公证或 Gatekeeper 信任。

首次在 macOS 终端执行（会要求设置 P12 导出密码，并可能弹出钥匙串授权）：

```sh
pnpm generate:p12     # 在仓库根目录生成或恢复 cliproam.p12
pnpm --filter @cliproam/app macos:check-signing
pnpm dev:app:mac       # 构建、校验并启动签名的 debug .app
pnpm build:app:mac     # 使用同一证书构建 release 安装包
```

证书备份保存在 `~/.config/cliproam/signing/cliproam.p12`，不要提交到仓库；请备份此文件及导出密码。`--setup` 会优先恢复已有 P12，或修复已有证书的本机信任，不替换旧证书。同名新证书仍是不同身份，会使原有授权失效。在其他开发机上，将原始 P12 放到同一路径，再执行 `--setup` 导入。签名与校验命令也可直接通过 `bash apps/app/src-tauri/native/macos/sign.sh` 执行，不依赖 pnpm。

CI 沿用 `.github/workflows/release.yml` 中的共用桌面工作流。它调用 `scripts/mac-import-signing-cert.sh`，转入 `native/macos/sign.sh --import-ci`，从以下仓库 Secrets 导入同一张证书、配置临时钥匙串并执行实际签名校验，再设置 `APPLE_SIGNING_IDENTITY` 供 Tauri 打包：

- `APPLE_CERTIFICATE`：上述原始 P12 的 Base64 内容，可用 `base64 -i ~/.config/cliproam/signing/cliproam.p12 | pbcopy` 复制。
- `APPLE_CERTIFICATE_PASSWORD`：创建或导出该 P12 时设置的密码。

若 CI 提示 `Unknown format in import` 或 P12 校验失败，先在本机执行 `/usr/bin/openssl pkcs12 -in ~/.config/cliproam/signing/cliproam.p12 -noout`，输入导出密码验证原始文件，再重新复制上述 Base64 内容到 `APPLE_CERTIFICATE`。Secret 不能填写文件路径、PEM 文本或重复 Base64 编码的内容。新生成的 P12 使用显式指定的钥匙串兼容算法；已有文件不会自动覆盖。

这两个 Secret 与更新器的 `TAURI_SIGNING_PRIVATE_KEY` 独立，后者继续负责更新包的验签。正式分发若使用 Developer ID，可通过 `APPLE_SIGNING_IDENTITY` 指定已导入的证书身份，并按 Tauri 文档另外配置公证。

VS Code 的 `Launch App · macOS · Signed` 使用签名 `.app`；LLDB 配置仍直接运行未经过整包签名的 `target/debug/cliproam`，用于原生调试，不应拿它验证安装版的授权。运行开发包前先退出安装版，避免单实例机制将操作转交给已经运行的应用。

从旧的未签名或 ad-hoc 安装包迁移时，需要退出应用、安装新签名版本，并在系统设置 → 隐私与安全性 → 辅助功能中删除旧条目，添加当前 `.app` 并开启权限。后续授权是否保留还受系统权限重置、证书变化等影响；签名不会自动授予权限。

排查当前安装包：

```sh
bash apps/app/src-tauri/native/macos/sign.sh --verify /Applications/ClipRoam.app
```

此检查要求完整签名校验通过，且签名要求不包含 `cdhash`。`CSSMERR_TP_NOT_TRUSTED` 表示当前机器不信任证书；本地自签名开发证书可运行 `--setup` 修复信任。当前已签名包需要手动重签时，先退出应用，再将 `.app` 路径直接传给脚本；不要在 Tauri 生成更新包及 `.sig` 后修改包内签名，否则应重新生成更新产物。

## macOS 图标

Dock 使用 `apps/icon/icon-fullbleed.png` 的不透明满版图案，不预先添加圆角或透明留白，由新版 macOS 处理图标外形。托盘使用 `apps/icon/icon.png`，按 18 pt 的菜单栏尺寸生成 2x 资源，并保留透明留白。

在仓库根目录执行 `pnpm sync:icons` 刷新全部图标；在 macOS 上会一并生成 Dock 与托盘资源。原生图标分别位于 `src-tauri/native/<平台>/icons/`，由各平台 Tauri 配置引用，生成文件不纳入 Git。Tauri 开发、打包以及直接 Cargo 构建会自动准备对应平台图标；更新 `.icns` 后，Cargo 会重新构建开发版以刷新嵌入的图标。
