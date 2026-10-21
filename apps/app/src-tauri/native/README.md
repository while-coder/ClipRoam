# 原生工程与平台资源

从这里查找 Windows、macOS、Linux、Android 和 iOS 的工程与配置。`native/` 保存需要维护的源码、资源和准备脚本；各平台的 `icons/` 和 `gen/` 是自动生成内容，不纳入 Git。

| 目录 | 用途 | 修改方式 |
| --- | --- | --- |
| `../gen/android/` | Android Studio / Gradle 主应用工程 | 由 `pnpm android:init` 生成并应用项目定制；不直接维护这里的文件。 |
| `../gen/apple/` | Xcode 主应用工程及生成的分享扩展 | 由 `pnpm ios:init` 生成并配置分享扩展；不直接维护这里的文件。 |
| `android/app/` | Android 主应用定制源码与配置 | 保存应用 Gradle（签名及 HTTP）、MainActivity（返回键处理）和显示名资源；准备脚本覆盖到生成工程。 |
| `android/prepare.cjs` | Android 工程准备脚本 | 复制定制文件并同步启动图标。其他工程文件由 Tauri 生成。 |
| `android/sync-icons.cjs` | Android 图标生成和工程资源同步 | 生成 `android/icons/` 并将其中的 `res/` 复制到生成工程。 |
| `windows/sync-icons.cjs` | Windows 图标生成 | 输出 `windows/icons/icon.ico`。 |
| `macos/sync-icons.cjs` | macOS 图标生成 | 输出窗口 PNG，并执行内嵌的 Swift 代码生成 Dock 和托盘图标。 |
| `linux/sync-icons.cjs` | Linux 图标生成 | 输出 32、128、256 像素 PNG。 |
| `ios/Info.plist` | iOS 主应用的附加配置 | 维护 iOS 的语言、HTTP ATS 和局域网权限；由 `tauri.ios.conf.json` 引用。 |
| `ios/locales/` | iOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`；准备脚本复制到主应用 target，由 Xcode 打包到应用根目录下的 `.lproj/`，不使用 Tauri 的 `bundle.resources`（会放进 `assets/`）。 |
| `ios/action-locales/` | iOS 分享面板中快捷操作的名称翻译 | 覆盖操作扩展的显示名；分享扩展沿用 `ios/locales/` 中的应用名称。两者均随 iOS 语言选择。 |
| `ios/action-icon.svg` | iOS 快捷操作的透明单色模板图标 | `ios/sync-icons.cjs` 生成多尺寸 AppIcon 资源并同步到操作扩展。 |
| `macos/Info.plist` | macOS 应用的附加配置 | 维护 macOS 的语言、HTTP ATS 和局域网权限；由 `tauri.macos.conf.json` 引用。 |
| `macos/locales/` | macOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`。 |
| `linux/linux.desktop` | Linux 桌面入口模板 | 维护启动命令、图标及本地化显示名；由 `tauri.linux.conf.json` 的 `bundle.linux.deb.desktopTemplate` 引用。 |
| `../share/plugin/ios/Sources/SharePlugin.swift` | iOS 分享扩展、操作扩展、主应用插件与共用收件箱 | `ios/prepare.cjs` 将文件复制到两个扩展工程并启用 `CLIPROAM_SHARE_EXTENSION`；Swift Package 编译主应用插件分支。 |
| `ios/prepare.cjs` | iOS 工程准备脚本 | 配置 App Group、分享扩展、操作扩展与共用的 Swift 收件箱源码；为关闭正式签名的设备构建添加打包前临时签名及权限检查。 |
| `ios/sync-icons.cjs` | iOS 图标生成和 Xcode 资源同步 | `prepare.cjs` 和根目录 `pnpm sync:icons` 调用。 |
| `../share/` | 分享接收与收件箱桥接 | Kotlin / Swift 保存内容并打开主 App，Rust 命令读取与确认收件箱，上传复用共用 TS 待上传队列。 |
| `../src/platforms/mobile/` | 主应用其他原生能力 | 剪贴板、文件选择与导出等 Rust 平台适配。 |

Tauri 的 `tauri.windows.conf.json`、`tauri.android.conf.json`、`tauri.ios.conf.json`、`tauri.macos.conf.json`、`tauri.linux.conf.json` 仍在 `src-tauri/` 根目录，供 CLI 按平台自动加载。图标路径由各平台配置中的 `bundle.icon` 指定。

在 `apps/app/` 执行：

- Android 首次初始化：`pnpm android:init`；后续使用 `pnpm android:dev` / `pnpm android:build`。
- iOS 首次初始化（macOS）：`pnpm ios:init`；后续使用 `pnpm ios:dev` / `pnpm ios:build`。

CI 在每次构建前分别调用 `pnpm android:init`、`pnpm ios:init`，流程为 `Tauri init → 平台准备脚本 → Tauri build`。开发和构建包装命令也会重新应用平台定制；不要把需要保留的修改只写在 `gen/` 中。仓库根目录可用 `pnpm --filter @cliproam/app android:init` 或 `pnpm --filter @cliproam/app ios:init`。

Android 显示名源文件位于 `android/app/src/main/res/values*/strings.xml`。签名密钥通过 CI Secrets/环境变量或本地 `apps/app/secrets/keystore.properties` 提供，不放进生成模板。`../share/plugin/android/` 是分享插件的 Android library 工程，与 `gen/android/` 的主应用工程分别由 Gradle 编译。

## 按平台生成图标

源图与生成配置保留在 `apps/icon/`。五个平台统一使用 `native/sync-icons.cjs → <平台>/sync-icons.cjs → 共用生成函数`：入口负责平台分发，共用函数负责缓存和 Tauri CLI 调用，各平台脚本负责输入选择、输出处理及移动端工程资源同步。各平台脚本也可以直接通过 `node native/<平台>/sync-icons.cjs` 运行（在 `src-tauri/` 目录下执行）。输入没有变化且输出完整时复用已有结果，平台脚本修改后也会重新生成：

| 目录 | 输出 |
| --- | --- |
| `windows/icons/` | Windows 安装包和窗口使用的 `icon.ico`。 |
| `macos/icons/` | 窗口 PNG、Dock 的 `icon.icns` 和托盘的 `tray-macos.png`；需要 macOS 运行 Swift 生成。 |
| `linux/icons/` | Linux 窗口和安装包使用的 32、128、256 像素 PNG。 |
| `android/icons/` | 原生上下文使用的 `icon.png` 与 `res/` 下的启动图标资源。 |
| `ios/icons/` | 原生上下文使用的 `icon.png` 与 `AppIcon-*.png` 资源。 |

这些目录均被 Git 忽略。生成缓存位于仓库 `.tmp/native-icons/`；缓存或任一输出丢失时会重新生成。

Tauri 开发和打包前会自动运行生成脚本；`build.rs` 也会在 `tauri_build::build()` 前按 Cargo 目标平台生成图标，因此现有 CI 和直接执行 `cargo build/check` 都不依赖预先提交的图标。运行前需安装 Node.js 和 pnpm 依赖。移动端初始化先准备对应图标，生成工程后再同步到原生资源目录。

根目录 `pnpm sync:icons` 仍刷新全部客户端资源，在 macOS 上同时生成 Dock 和托盘图标。旧的 `src-tauri/icons/` 不再使用。
