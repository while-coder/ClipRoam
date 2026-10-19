# 随心贴 Firefox 扩展

Manifest V3 弹出页和选项页通过 `NativeFirefox` 加载共享的 `@cliproam/ui/app`，共用登录、历史、设置、同步和传输逻辑。

- 开发预览：`pnpm --filter @cliproam/firefox dev`（端口 1433）。
- 构建：根目录运行 `pnpm build:firefox`。
- 临时加载：在 Firefox 128 或更新版本打开 `about:debugging#/runtime/this-firefox`，点击“临时载入附加组件”，选择 `apps/firefox/dist/manifest.json`。
- Release 产物：`ClipRoam_版本号_firefox.zip`，解压后按上述方式加载。

插件申请剪贴板读写权限，以及 HTTP/HTTPS 同步服务器的访问权限。使用手动读取剪贴板，不在后台监听剪贴板变化。

当前生成未签名的开发包；Firefox 正式版长期安装需要由 Mozilla 签名的 XPI，不能直接将 ZIP 改后缀作为正式安装包。
