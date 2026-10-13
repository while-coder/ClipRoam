# ClipRoam 共用客户端

`@cliproam/ui` 包含登录、历史、设置、设备、上传与下载页面，以及账号状态、同步、捕获、缓存和传输业务。三个入口加载同一个 `App.vue`，通过 `NativePlatform` 调用各平台能力。

客户端先调用 `configureNative`，再动态导入 `@cliproam/ui/app`，确保共享状态初始化时已取得正确适配器。`@cliproam/ui/native` 导出接口与初始化函数；`@cliproam/ui/native-web` 提供共用 Web API 实现，PWA 和 Chrome 各自扩展它。

Tauri SDK、日志、原生更新器、快捷粘贴窗口、独立 Toast 窗口及其样式保留在 `apps/app`。快速粘贴视图和原生更新弹窗通过共享 App 的 `quick-paste`、`native-overlays` 插槽接入。独立 Toast 入口不加载共享业务 App。

组件直接导出源码，由各入口的 Vite 构建。各入口安装 Vue 与 ui-kit 插件，并导入 `@cliproam/ui/styles.css`。工作区布局和展示组件仍可从包的主入口独立引用。
