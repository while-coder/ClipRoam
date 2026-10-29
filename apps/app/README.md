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

## macOS 签名与辅助功能授权

macOS 默认使用固定自签名证书 `cliproam-dev` 签名整个 `.app`。Bundle ID 保持 `com.whilecode.cliproam`；持续使用同一张证书，使签名要求不再绑定每次构建变化的 `cdhash`。缺少证书时构建失败，不自动降级为 ad-hoc 签名。自签名证书不提供 Apple 公证或 Gatekeeper 信任。

首次在 macOS 终端执行（会要求设置 P12 导出密码，并可能弹出钥匙串授权）：

```sh
pnpm --filter @cliproam/app macos:setup-signing
pnpm --filter @cliproam/app macos:check-signing
pnpm dev:app:mac       # 构建、校验并启动签名的 debug .app
pnpm build:app:mac     # 使用同一证书构建 release 安装包
```

证书备份保存在 `~/.config/cliproam/signing/cliproam-dev.p12`，不要提交到仓库；请备份此文件及导出密码。`--setup` 会优先恢复已有 P12，或修复已有证书的本机信任，不替换旧证书。同名新证书仍是不同身份，会使原有授权失效。在其他开发机上，将原始 P12 放到同一路径，再执行 `--setup` 导入。签名与校验命令也可直接通过 `bash apps/app/src-tauri/native/macos/sign.sh` 执行，不依赖 pnpm。

CI 沿用 `.github/workflows/release.yml` 中的共用桌面工作流。它调用 `scripts/mac-import-signing-cert.sh`，转入 `native/macos/sign.sh --import-ci`，从以下仓库 Secrets 导入同一张证书、配置临时钥匙串并执行实际签名校验，再设置 `APPLE_SIGNING_IDENTITY` 供 Tauri 打包：

- `APPLE_CERTIFICATE`：上述原始 P12 的 Base64 内容，可用 `base64 -i ~/.config/cliproam/signing/cliproam-dev.p12 | pbcopy` 复制。
- `APPLE_CERTIFICATE_PASSWORD`：创建或导出该 P12 时设置的密码。

这两个 Secret 与更新器的 `TAURI_SIGNING_PRIVATE_KEY` 独立，后者继续负责更新包的验签。正式分发若使用 Developer ID，可通过 `APPLE_SIGNING_IDENTITY` 指定已导入的证书身份，并按 Tauri 文档另外配置公证。

VS Code 的 `Launch App · macOS · Signed` 使用签名 `.app`；LLDB 配置仍直接运行未经过整包签名的 `target/debug/cliproam`，用于原生调试，不应拿它验证安装版的授权。运行开发包前先退出安装版，避免单实例机制将操作转交给已经运行的应用。

从旧的未签名或 ad-hoc 安装包迁移时，需要退出应用、安装新签名版本，并在系统设置 → 隐私与安全性 → 辅助功能中删除旧条目，添加当前 `.app` 并开启权限。后续授权是否保留还受系统权限重置、证书变化等影响；签名不会自动授予权限。

排查当前安装包：

```sh
bash apps/app/src-tauri/native/macos/sign.sh --verify /Applications/ClipRoam.app
```

此检查要求完整签名校验通过，且签名要求不包含 `cdhash`。`CSSMERR_TP_NOT_TRUSTED` 表示当前机器不信任证书；本地自签名开发证书可运行 `--setup` 修复信任。当前已签名包需要手动重签时，先退出应用，再将 `.app` 路径直接传给脚本；不要在 Tauri 生成更新包及 `.sig` 后修改包内签名，否则应重新生成更新产物。

## macOS 图标

Dock 与托盘沿用 `apps/icon/icon.png` 的同一图案。Dock 主体约占画布宽度的 84%，托盘按 18 pt 的菜单栏尺寸生成 2x 资源，并保留透明留白。

在仓库根目录执行 `pnpm sync:icons` 刷新全部图标；在 macOS 上会一并生成 Dock 与托盘资源。原生图标分别位于 `src-tauri/native/<平台>/icons/`，由各平台 Tauri 配置引用，生成文件不纳入 Git。Tauri 开发、打包以及直接 Cargo 构建会自动准备对应平台图标；更新 `.icns` 后，Cargo 会重新构建开发版以刷新嵌入的图标。
