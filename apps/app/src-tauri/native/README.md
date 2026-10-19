# 原生工程与平台资源

从这里查找 Android、iOS 和 macOS 的工程与配置。`native/` 保存需要维护的源码、资源和准备脚本；`gen/` 是 CI 初始化时创建并实际参与构建的工程，不纳入 Git。

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
| `ios/ShareExtension/` | iOS 分享扩展的 Swift 生命周期入口 | `prepare.cjs` 复制到 Xcode 工程；上传实现位于 `../share/`。 |
| `ios/prepare.cjs` | iOS 工程准备与扩展 Rust 构建脚本 | 默认配置 App Group、Keychain、分享扩展及静态库链接；Xcode 构建阶段调用 `build-share-core` 子命令编译 Rust 静态库。 |
| `ios/sync-icons.cjs` | iOS 图标生成和 Xcode 资源同步 | `prepare.cjs` 和根目录 `pnpm sync:icons` 调用。 |
| `../share/` | 分享功能的 Rust 核心和原生桥接 | 共用协议在 `core/`；Android 适配在 `android.rs`；Kotlin 入口与 iOS FFI 在 `plugin/`。 |
| `../src/platforms/mobile/` | 主应用其他原生能力 | 剪贴板、文件选择与导出等 Rust 平台适配。 |

Tauri 的 `tauri.android.conf.json`、`tauri.ios.conf.json`、`tauri.macos.conf.json` 仍在 `src-tauri/` 根目录，供 CLI 按平台自动加载。图标仍统一存放于 `../icons/`。

在 `apps/app/` 执行：

- Android 首次初始化：`pnpm android:init`；后续使用 `pnpm android:dev` / `pnpm android:build`。
- iOS 首次初始化（macOS）：`pnpm ios:init`；后续使用 `pnpm ios:dev` / `pnpm ios:build`。

CI 在每次构建前分别调用 `pnpm android:init`、`pnpm ios:init`，流程为 `Tauri init → 平台准备脚本 → Tauri build`。开发和构建包装命令也会重新应用平台定制；不要把需要保留的修改只写在 `gen/` 中。仓库根目录可用 `pnpm --filter @cliproam/app android:init` 或 `pnpm --filter @cliproam/app ios:init`。

Android 显示名源文件位于 `android/app/src/main/res/values*/strings.xml`。签名密钥通过 CI Secrets/环境变量或本地 `apps/app/secrets/keystore.properties` 提供，不放进生成模板。`../share/plugin/android/` 是分享插件的 Android library 工程，与 `gen/android/` 的主应用工程分别由 Gradle 编译。
