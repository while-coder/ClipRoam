# ClipRoam PWA 壳

入口注入 `NativePwa` 后加载 `@cliproam/ui/app`，共用登录、历史、设置、同步与传输逻辑。`NativePwa` 继承共用 Web API 适配器，后续 PWA 专有能力在本项目扩展。

目前尚未加入 Web App Manifest、Service Worker 或离线安装功能。

- 开发：`pnpm --filter @cliproam/pwa dev`
- 构建：`pnpm --filter @cliproam/pwa build`
