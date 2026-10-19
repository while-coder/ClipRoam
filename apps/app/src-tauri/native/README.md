# 原生工程与平台资源

从这里查找 Android、iOS、macOS 和 Linux 的工程与配置。`native/` 保存需要维护的源码、资源和准备脚本；`gen/` 是 CI 初始化时创建并实际参与构建的工程，不纳入 Git。

| 目录 | 用途 | 修改方式 |
| --- | --- | --- |
| `../gen/android/` | Android Studio / Gradle 主应用工程 | 由 `pnpm android:init` 生成并应用项目定制；不直接维护这里的文件。 |
| `../gen/apple/` | Xcode 主应用工程及生成的分享扩展 | 由 `pnpm ios:init` 生成并配置分享扩展；不直接维护这里的文件。 |
| `android/app/` | Android 主应用定制源码与配置 | 保存应用 Gradle（签名及 HTTP）、MainActivity（返回键处理）和显示名资源；准备脚本覆盖到生成工程。 |
| `android/prepare.cjs` | Android 工程准备脚本 | 复制定制文件并同步启动图标。其他工程文件由 Tauri 生成。 |
| `android/sync-icons.cjs` | Android 启动图标生成 | 根据 `apps/icon/icons.json` 生成资源并复制到工程；初始化和根目录 `pnpm sync:icons` 都会调用。 |
| `ios/Info.plist` | iOS 主应用的附加配置 | 维护 iOS 的语言、HTTP ATS 和局域网权限；由 `tauri.ios.conf.json` 引用。 |
| `ios/locales/` | iOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`。 |
| `macos/Info.plist` | macOS 应用的附加配置 | 维护 macOS 的语言、HTTP ATS 和局域网权限；由 `tauri.macos.conf.json` 引用。 |
| `macos/locales/` | macOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`。 |
| `macos/mac-icon.swift` | macOS 应用与托盘图标生成 | 根目录 `pnpm sync:icons` 在 macOS 上调用，输出 `icons/icon.icns` 和 `icons/tray-macos.png`。 |
| `linux/linux.desktop` | Linux 桌面入口模板 | 维护启动命令、图标及本地化显示名；由 `tauri.linux.conf.json` 的 `bundle.linux.deb.desktopTemplate` 引用。 |
| `../share/plugin/ios/Sources/SharePlugin.swift` | iOS 分享扩展、主应用插件与共用收件箱 | `ios/prepare.cjs` 将文件复制到扩展工程并启用 `CLIPROAM_SHARE_EXTENSION`；Swift Package 编译主应用插件分支。 |
| `ios/prepare.cjs` | iOS 工程准备脚本 | 配置 App Group、分享扩展与共用的 Swift 收件箱源码。 |
| `ios/sync-icons.cjs` | iOS 图标生成和 Xcode 资源同步 | `prepare.cjs` 和根目录 `pnpm sync:icons` 调用。 |
| `../share/` | 分享接收与收件箱桥接 | Kotlin / Swift 保存内容并打开主 App，Rust 命令读取与确认收件箱，上传复用共用 TS 待上传队列。 |
| `../src/platforms/mobile/` | 主应用其他原生能力 | 剪贴板、文件选择与导出等 Rust 平台适配。 |

Tauri 的 `tauri.android.conf.json`、`tauri.ios.conf.json`、`tauri.macos.conf.json`、`tauri.linux.conf.json` 仍在 `src-tauri/` 根目录，供 CLI 按平台自动加载。图标仍统一存放于 `../icons/`。

在 `apps/app/` 执行：

- Android 首次初始化：`pnpm android:init`；后续使用 `pnpm android:dev` / `pnpm android:build`。
- iOS 首次初始化（macOS）：`pnpm ios:init`；后续使用 `pnpm ios:dev` / `pnpm ios:build`。

CI 在每次构建前分别调用 `pnpm android:init`、`pnpm ios:init`，流程为 `Tauri init → 平台准备脚本 → Tauri build`。开发和构建包装命令也会重新应用平台定制；不要把需要保留的修改只写在 `gen/` 中。仓库根目录可用 `pnpm --filter @cliproam/app android:init` 或 `pnpm --filter @cliproam/app ios:init`。

Android 显示名源文件位于 `android/app/src/main/res/values*/strings.xml`。签名密钥通过 CI Secrets/环境变量或本地 `apps/app/secrets/keystore.properties` 提供，不放进生成模板。`../share/plugin/android/` 是分享插件的 Android library 工程，与 `gen/android/` 的主应用工程分别由 Gradle 编译。
