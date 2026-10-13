# ClipRoam 客户端

Vue + TypeScript 实现界面和业务逻辑，Tauri 提供原生能力。主窗口与快速粘贴共用一个原生窗口，由 TS 切换标题栏、尺寸、位置和置顶状态。

## 代码边界

- `src/core`：账号数据、历史缓存、内容摘要、捕获过滤、文件哈希、待上传队列、下载调度、下载记录和 ZIP 导出。
- `src/features`：Vue 状态与业务服务接入、服务器 HTTP/WebSocket 同步、上传和设置。
- `src/platform`：文件、剪贴板、登录存储等平台适配。浏览器使用 IndexedDB Blob、文件选择器、下载和剪贴板 API；桌面通过 Tauri 调用原生接口。
- `src-tauri/src`：统一函数入口和所有 `native_` 前缀的 Tauri 命令；`platform.rs` 显式声明跨平台能力，`platforms/` 存放桌面、移动端及各系统的具体实现。
- 原生能力：系统剪贴板、图片格式适配、焦点与粘贴按键、文件字节读写、系统对话框、Android 分享/文件导出、窗口与托盘。Rust 不再维护账号数据库、同步客户端或下载队列。

文件导出由 TS 遍历目录并调用 `native_fs_mkdir`、`native_fs_copy`；传入父目录和 `name` 时，命令返回实际创建的路径或 URI，后续操作使用该返回值。Android 系统目录 URI 由平台实现处理，失败或取消后 TS 清理本次创建的根子项；原生层不再提供整目录导出命令。

## 本地数据

登录配置只包含服务器地址及协议、用户名和 sessionToken，使用 AES-GCM 加密后保存到 localStorage，每次保存使用新的随机 IV。随机密钥保存在同一应用的 localStorage，目的是避免直接保存明文；这不提供系统钥匙串级别的隔离。退出登录删除登录配置，配置或密钥损坏后可重新登录覆盖。

历史缓存、待上传队列、账号偏好、设备标识和下载记录使用 IndexedDB，账号数据按服务器与用户名隔离。桌面文件内容保存在应用目录，浏览器文件内容保存为 IndexedDB Blob。没有旧 SQLite、旧配置或钥匙串数据导入流程；升级后需要重新登录，历史从服务器重新获取。

## 验证与浏览器边界

在仓库根目录执行 `pnpm --filter @cliproam/app build` 和 `pnpm --filter @cliproam/app test`；在 `src-tauri` 执行 `cargo test --offline`。

业务核心可供 PWA 或浏览器插件复用，当前尚未加入 PWA manifest、Service Worker 或插件打包。浏览器不提供全局快捷键、后台系统剪贴板监听、向其他应用投递粘贴按键和系统文件剪贴板；读取或写入剪贴板仍受浏览器权限约束。
