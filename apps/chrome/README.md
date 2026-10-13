# ClipRoam Chrome 扩展占位项目

引用 `@cliproam/ui`，提供 Manifest V3 弹出页和选项页。目前仅包含共用布局及导航，不申请浏览器权限，不包含后台服务、剪贴板或账号业务。

- 开发预览：`pnpm --filter @cliproam/chrome dev`
- 构建：`pnpm --filter @cliproam/chrome build`
- 本地加载：在 Chrome 扩展管理页开启开发者模式，加载 `apps/chrome/dist` 为已解压的扩展。
