# ClipRoam PWA 壳

入口注入 `NativePwa` 后加载 `@cliproam/ui/app`，共用登录、历史、设置、同步与传输逻辑。`NativePwa` 继承共用 Web API 适配器，后续 PWA 专有能力在本项目扩展。

包含 Web App Manifest、应用图标和接收系统分享的 Service Worker。通过 Android Chrome 安装 PWA 后，可在其他应用的系统分享菜单中选择「ClipRoam」，接收文字、链接、图片和多个文件；图片作为文件导入。分享内容先保存在 IndexedDB，登录后进入当前账号的待上传列表，导入成功才清理，失败或未登录时保留。

分享接收需要 HTTPS（本机 localhost 除外）及支持 Web Share Target 的浏览器；iOS Safari 不支持。Service Worker 在生产构建中注册，使用 `pnpm --filter @cliproam/pwa build` 后运行 `pnpm --filter @cliproam/pwa preview` 验证。子目录部署时设置 `PWA_BASE=/ClipRoam/`。当前不提供完整离线缓存。

- 开发：`pnpm --filter @cliproam/pwa dev`
- 构建：`pnpm --filter @cliproam/pwa build`
