# 移动端分享

接收系统分享、保存待处理内容，并在已有登录状态时后台上传。主应用通过 Tauri 命令读取和确认待处理分享。

```text
share/
  mod.rs, inbox.rs             # 主应用命令与待处理分享导入
  core/src/
    lib.rs                    # 哈希、分片、发布及唯一 JSON / C 入口
    auth.rs                   # 共用配置文件与加解密
    mobile.rs                 # 两端共用的收件箱、任务配置与恢复命令
    upload.rs                 # 共用任务推进与 Android Rust HTTP 执行器
  android/mod.rs              # 唯一 JNI 入口，只转发 JSON
  plugin/
    src/lib.rs                # 主应用 Tauri 插件接入
    android/                  # Kotlin 分享接收、文件暂存与 Worker 管理
    ios/
      ShareExtension/         # Swift 分享接收与文件暂存
      Sources/Support/        # 唯一 C 入口封装与原生 URLSession HTTP 接口
      Sources/Plugin/         # 主应用插件与后台 URLSession 生命周期接入
      FFI/                    # C 接口声明
```

Rust 上传核心不依赖 JNI、Objective-C、UIKit 或 WorkManager。原生层接收分享并暂存文件，将文本、文件路径和配置目录一次传入；服务器配置、任务快照、哈希、分片、请求生成、响应解析和上传进度均在 Rust 中处理。原生收件箱与主应用目录分别由平台提供，不为每项文件操作增加 Rust 回调。

Android 的 Kotlin 入口通过唯一的 `ShareNative.call(JSON)` 调用 Rust。`stage` / `resume` 返回任务 ID，Kotlin 使用 WorkManager 调度 Worker；Worker 调用 `upload` 后，Rust 使用内部 HTTP 客户端循环处理请求，返回 `success` / `retry` / `failed`，由 Kotlin 映射为 Worker 结果。分享界面在任务成功入队后关闭。

iOS 的 Swift 入口通过唯一的 `cliproam_share_call(JSON)` 调用同一 Rust 核心。`step` 返回 HTTP 请求（URL、方法、请求头、请求体文件），Swift 的 URLSession 接口只负责提交请求；收到结果后通过 `response` 交回 Rust，取得下一条请求，直到 `request` 为 null。分享扩展提交第一条后台请求后即可关闭，主应用通过相同会话标识接回系统事件并继续推进。Swift 不保存 Rust 指针或等待中的 Rust 函数栈。

两端均在每次响应后保存进度，网络或服务器故障保留任务；上传限制等无法自动处理的情况回到主应用待上传流程。HTTP 请求体始终写入文件，满足 iOS 后台上传要求。Android 的内置 HTTP 客户端使用标准 WebPKI 根证书验证 HTTPS，不跳过证书验证。

## 分享上传配置

启动并成功登录或恢复账号后，主应用将服务器 URL、session token、用户名、设备信息（ID、名称、平台、系统及应用版本）、账号范围和上传限制写入 `files/share-auth/current`。Android 使用应用私有目录，iOS 使用主应用与分享扩展共用的 App Group 容器。退出或切换账号时删除当前配置。

两端通过 `core/src/auth.rs` 共用文件读写、AES-256-GCM 加解密和 JSON 解析，平台只提供目录。密钥固定在 Rust 中，每次写入使用随机 nonce；这用于避免直接保存明文，不提供 KeyStore / Keychain 级别的密钥隔离。更新后启动并登录会写入新格式，不读取旧版 KeyStore / Keychain 凭据。

分享入口只需读取 `current` 即可取得完整上传配置。已创建的任务保留独立的 `share-auth/<任务ID>` 配置快照，避免切换账号后串用凭据；这些文件使用相同格式。上传计划不包含 token。

原生工程准备脚本保留在 `../native/android/` 和 `../native/ios/`。iOS 准备脚本将 `plugin/ios/ShareExtension/` 的入口复制到生成的 Xcode 工程，并构建 `core/` 静态库。生成工程位于 `../gen/`，不直接维护。
