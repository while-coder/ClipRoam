# 移动端分享

接收系统分享、保存待处理内容，并在已有登录状态时后台上传。主应用通过 Tauri 命令读取和确认待处理分享。

```text
share/
  mod.rs, inbox.rs             # 主应用命令与待处理分享导入
  core/                       # 共用上传状态机与静态库入口
  android/
    mod.rs                    # Intent、KeyStore、WorkManager 与 HTTP 适配
    jni.rs                    # Kotlin 调用 Rust 的 JNI 入口
  ios/
    mod.rs                    # 分享内容、App Group、Keychain 与 URLSession 适配
  plugin/
    src/lib.rs                # Tauri 插件注册与平台调用
    android/                  # Kotlin 分享 Activity、Worker 与插件工程
    ios/
      ShareExtension/         # Swift 分享扩展生命周期入口
      FFI/                    # Swift 调用 Rust 的 C 接口声明
```

Android 的 Rust 适配由主应用的 `share::android` 模块编译，Kotlin 入口加载主应用动态库。iOS 的 Rust 适配通过 `core/src/lib.rs` 的路径声明编译为 `cliproam_share_upload::ios`，供主应用和独立分享扩展使用；扩展无需链接整个 Tauri 主应用。

原生工程准备脚本保留在 `../native/android/` 和 `../native/ios/`。iOS 准备脚本将 `plugin/ios/ShareExtension/` 的入口复制到生成的 Xcode 工程，并构建 `core/` 静态库。生成工程位于 `../gen/`，不直接维护。
