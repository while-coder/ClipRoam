# 原生工程与平台资源

从这里查找 Android、iOS 和 macOS 的工程与配置。`gen/` 是 Tauri 创建并实际参与构建的工程；`native/` 保存需要独立维护的资源和 iOS 工程准备脚本。

| 目录 | 用途 | 修改方式 |
| --- | --- | --- |
| `../gen/android/` | Android Studio / Gradle 主应用工程 | 已纳入 Git；直接维护 Manifest、MainActivity、Gradle 和资源。不要当成缓存删除，重新 init 后须检查自定义配置。 |
| `../gen/apple/` | Xcode 主应用工程及生成的分享扩展 | 未纳入 Git；由 `pnpm ios:init` 创建，`ios/prepare.cjs` 补充扩展、权限和构建设置。持久修改应写进源配置或准备脚本。 |
| `ios/Info.plist` | iOS 主应用的附加配置 | 维护 iOS 的语言、HTTP ATS 和局域网权限；由 `tauri.ios.conf.json` 引用。 |
| `ios/locales/` | iOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`。 |
| `macos/Info.plist` | macOS 应用的附加配置 | 维护 macOS 的语言、HTTP ATS 和局域网权限；由 `tauri.macos.conf.json` 引用。 |
| `macos/locales/` | macOS 系统显示名及权限说明翻译 | 独立维护各语言的 `InfoPlist.strings`。 |
| `ios/ShareExtension/` | iOS 分享扩展的 Swift 生命周期入口 | `prepare.cjs` 复制到 Xcode 工程；上传实现位于 `../share/`。 |
| `ios/prepare.cjs` | iOS 工程准备脚本 | 配置 App Group、Keychain、分享扩展及 Rust 静态库链接。 |
| `ios/build-share-core.sh` | iOS 扩展的 Rust 静态库构建 | Xcode 构建阶段调用。 |
| `ios/sync-icons.cjs` | iOS 图标生成和 Xcode 资源同步 | `prepare.cjs` 和根目录 `pnpm sync:icons` 调用。 |
| `../share/` | 分享功能的 Rust 核心和原生桥接 | 共用协议在 `core/`；Android 适配在 `android.rs`；Kotlin 入口与 iOS FFI 在 `plugin/`。 |
| `../src/platforms/mobile/` | 主应用其他原生能力 | 剪贴板、文件选择与导出等 Rust 平台适配。 |

Tauri 的 `tauri.android.conf.json`、`tauri.ios.conf.json`、`tauri.macos.conf.json` 仍在 `src-tauri/` 根目录，供 CLI 按平台自动加载。图标仍统一存放于 `../icons/`。

在 `apps/app/` 执行：

- Android：`pnpm android:dev` / `pnpm android:build`。
- iOS 首次初始化：`pnpm ios:init`；后续使用 `pnpm ios:dev` / `pnpm ios:build`，会先准备 Xcode 工程。

Android 显示名位于 `../gen/android/app/src/main/res/values*/strings.xml`。`../share/plugin/android/` 是分享插件的 Android library 工程，与 `gen/android/` 的主应用工程分别由 Gradle 编译。
