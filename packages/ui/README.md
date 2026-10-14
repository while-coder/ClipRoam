# ClipRoam 共用客户端

`@cliproam/ui` 包含登录、历史、设置、设备、上传与下载页面，以及账号状态、同步、捕获、缓存和传输业务。三个入口加载同一个 `App.vue`，通过 `NativePlatform` 调用各平台能力。

客户端先调用 `configureNative`，再动态导入 `@cliproam/ui/app`，确保共享状态初始化时已取得正确适配器。`@cliproam/ui/native` 导出接口与初始化函数；`@cliproam/ui/native-web` 提供共用 Web API 实现，PWA 和 Chrome 各自扩展它。

Tauri SDK、日志、原生更新器、快捷粘贴窗口、独立 Toast 窗口及其样式保留在 `apps/app`。快速粘贴视图和原生更新弹窗通过共享 App 的 `quick-paste`、`native-overlays` 插槽接入。独立 Toast 入口不加载共享业务 App。

组件直接导出源码，由各入口的 Vite 构建。各入口安装 Vue 与 ui-kit 插件，并导入 `@cliproam/ui/styles.css`。工作区布局和展示组件仍可从包的主入口独立引用。

## UI 代码结构

`src/features` 按功能组织代码，功能内部区分状态管理、业务服务和页面绑定：

| 功能 | 共享 Manager | 职责 |
| --- | --- | --- |
| `history` | `HistoryManager` | 所有记录的本地读取、历史分页、详情回填、缓存、发布记录合并、删除与刷新通知 |
| `pending-upload` | `PendingUploadManager` | 持久化待上传队列、列表与数量、失败重试、文件解析结果、上传进度 |
| `uploads` | `UploadManager` | 本机中继上传任务列表、状态与进度、活动数量、终态任务淘汰 |
| `downloads` | `DownloadManager` | 下载队列、批次去重与取消、进度、下载记录持久化、另存为及条目下载状态 |

每个账号会话创建一组 Manager，入口是 `getAccountSession()` 的 `history`、`pendingUploads`、`uploads` 和 `downloads`。它们在应用内共享，页面切换不会重建；退出或切换账号时，会话取消任务并释放资源。

- `*Manager.ts` 拥有列表状态和业务操作，不依赖页面组件或 `use*` 页面绑定。
- `use*.ts` 绑定当前账号的响应式状态，处理界面提示、焦点及页面交互。
- `.vue` 文件负责布局、显示和用户事件。历史页的筛选、选中项与页码属于视图状态，不共享给其他历史视图。
- `HistoryClient` 负责历史 HTTP 请求及详情回填，`HistoryManager` 直接管理记录的本地缓存和刷新通知。
- `CaptureService` 捕获和解析剪贴板；`PendingUploader` 发布待上传记录；`RelayUploader` 传输中继文件。它们通过对应 Manager 读写队列或任务列表。
- `src/accountSession.ts` 装配业务 Manager，创建账号与设置状态，管理账号资源、取消信号和定时任务。`ClientSessionManager` 统一管理当前账号配置、偏好、连接状态、设备列表和会话启停；WebSocket、心跳、重连与传输 worker 装配收在同文件的私有连接类中。
- `sync/httpClient.ts` 提供认证 HTTP 请求、取消与响应校验；每个账号连接持有一个请求器，历史、上传和下载通过所属会话复用它。`sync/authApi.ts` 保留登录、注册、会话验证、设备信息更新和改密 API。
- `src/accountSession.ts` 管理账号状态与资源生命周期；`src/utils.ts` 放置共享工具函数，`src/types.ts` 放置共享类型；`src/platform` 封装数据库、平台操作、通知与能力状态；`src/components` 放置跨功能的展示组件。
- `src/config.ts` 统一平台能力、服务器地址、分页和账号偏好的默认配置；登录表单与认证逻辑放在 `features/sync`。
- `WorkspacePage` 统一页面布局，业务页面通过 `menu` 插槽放入 `WorkspaceMenu`；包主入口导出的布局默认不带业务菜单。
- `platform/runtime.ts` 统一窗口模式、平台能力和版本信息；`usePendingUploads.ts` 统一手动捕获与待上传列表交互；下载快照类型和取消异常由 `downloadManager.ts` 导出。

目录保持按职责和业务功能划分，不再为单个类型、状态绑定或简单分页标记创建独立文件：

```text
src/
  App.vue, config.ts, types.ts, styles.css
  index.ts, host.ts                 # 包公开入口
  assets/                          # 图片资源
  components/                      # 跨功能布局和展示组件
  accountSession.ts                # 账号状态、资源与任务生命周期
  platform/                        # 平台接口、适配器、运行信息、文件与存储
  utils.ts                         # 取消信号、字节转换、格式化、内容处理与哈希
  features/
    sync/                          # ClientSessionManager、HTTP、认证 API、登录表单
    history/                       # 历史缓存、分页查询、筛选和预览
    pending-upload/                # 捕获、持久队列、发布和文件上传
    uploads/                       # 中继上传任务
    downloads/                     # 下载与另存为任务
    settings/                      # 设置交互和桌面、移动端弹窗
    devices/                       # 设备页面
```

新增列表操作应放到所属 Manager，页面绑定只调用 Manager，不直接读写数据库或维护另一份业务列表。

通用纯函数集中在 `src/utils.ts`，按用途分组并保留具名导出。`@cliproam/ui/bytes` 和 `@cliproam/ui/errors` 映射到该模块，原有客户端导入方式保持可用。账号生命周期由 `src/accountSession.ts` 管理；数据库与提示归 `src/platform`，剪贴板内容处理和增量哈希归 `src/utils.ts`。

客户端会话从 `clientSessionManager` 单例进入，`start` 创建账号资源与连接，`stop` 释放账号资源；配置与偏好持久化也由该入口提供。登录验证成功与 WebSocket 已连接是两个独立状态，连接暂时断开不会清除登录状态。后台任务持有自己的 `AccountSession`，通过 `session.http` 使用该账号的请求器；账号切换会取消旧会话，旧任务不能取得新账号的凭证。

类型检查：`pnpm --filter @cliproam/ui check`。
