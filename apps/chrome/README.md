# 随心贴 Chrome 扩展壳

Manifest V3 弹出页和选项页注入 `NativeChrome` 后加载 `@cliproam/ui/app`，共用登录、历史、设置、同步与传输逻辑。`NativeChrome` 继承共用 Web API 适配器，后续扩展专有 API 在本项目接入。

目前未申请额外浏览器权限，也没有后台服务。没有 Tauri 快捷粘贴或独立 Toast 窗口。

- 开发预览：`pnpm --filter @cliproam/chrome dev`
- 构建：`pnpm --filter @cliproam/chrome build`
- 本地加载：在 Chrome 扩展管理页开启开发者模式，加载 `apps/chrome/dist` 为已解压的扩展。
