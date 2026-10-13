# ClipRoam 共用 UI

`@cliproam/ui` 提供 `WorkspaceLayout`（主界面、桌面侧栏、移动导航、快捷粘贴布局）、`WorkspacePage`（页面骨架）、`EntryKindIcon`、`TransferProgress` 和共用样式。

三个客户端通过 props、事件和插槽接入页面与业务状态；本包不依赖客户端目录、Tauri、账号会话或网络同步。主入口导入组件，`@cliproam/ui/styles.css` 导入样式。Vue 与 ui-kit 插件由各客户端入口安装。

组件直接导出源码，由各客户端的 Vite 构建。`apps/app` 的工作区菜单通过 `menu` 插槽接入，设置弹窗通过布局的 `overlays` 插槽接入。
