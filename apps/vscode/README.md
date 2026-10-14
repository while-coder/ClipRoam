# ClipRoam VS Code 扩展

在编辑器标签页中使用 ClipRoam，共用登录、历史、设置与同步业务。需要连接已部署的 ClipRoam Server。

## 使用

安装 VSIX 后，在命令面板运行 `ClipRoam: 打开剪贴板历史`，填写服务器地址并登录。

- **保存选中文本**：选中编辑器文本，右键选择 `ClipRoam: 保存选中文本`，或使用 ClipRoam 标签页顶部的按钮。文本加入当前账号的上传队列。
- **插入历史文本**：右键选择 `ClipRoam: 从历史插入文本`，双击历史文本或使用复制菜单，再点击顶部的“插入已复制文本”。内容替换最近使用的文本编辑器的当前选区，支持多个光标。普通复制仍只写入系统剪贴板。
- **读取剪贴板**：使用历史界面的“读取当前剪贴板”操作。

首次安装或重新打开标签页时，需要等待登录验证完成再保存文本。登录和缓存沿用共享客户端的 Webview IndexedDB。关闭标签页后同步停止，重新打开会恢复保存的连接设置并验证会话。

首版支持桌面 VS Code，使用本地 UI 扩展宿主；没有浏览器版扩展入口。仅支持文本剪贴板，不监听系统复制，不提供全局快捷键；图片和文件可以查看、传输到应用缓存，系统图片/文件复制和原生文件导出暂不支持。

## 构建和调试

在仓库根目录执行：

```bash
pnpm install
pnpm build:vscode
pnpm package:vscode
```

VSIX 位于 `apps/vscode/dist/cliproam.vsix`。在 VS Code 扩展菜单中选择“从 VSIX 安装”。开发时选择仓库的 `Launch VS Code Extension` 调试配置，按 F5 启动扩展开发宿主。

构建后可运行 `pnpm --filter cliproam-vscode test`，验证扩展宿主消息桥、选区插入和标签页关闭清理。测试使用模拟 VS Code API；真实窗口的登录、同步和 Webview 行为需要在扩展开发宿主中验证。

扩展通过受限消息桥调用 VS Code 文本剪贴板、文本编辑器和外部链接 API。Webview 仅允许加载打包资源，HTTP/WebSocket 连接用于访问用户配置的服务器。
