/**
 * 前端日志：把浏览器 console 调用转发到 tauri-plugin-log，
 * 使前端日志与 Rust 端写入同一个文件（系统日志目录），追 BUG 时前后端连贯。
 * 原始 console 行为保留，devtools 仍可见；业务代码无需改写法。
 *
 * 级别映射（dev=Debug / release=Info 由 Rust 端 logging_plugin 控制）：
 *   console.log/debug -> debug，console.info -> info，console.warn -> warn，console.error -> error。
 * 因此 dev 下 log/debug 也会落文件，release 下仅 info 及以上落文件。
 *
 * 在 main.ts 启动时调用一次 setupLogger()。
 */
import { debug, info, warn, error } from '@tauri-apps/plugin-log'

type ConsoleMethod = 'log' | 'debug' | 'info' | 'warn' | 'error'
type Logger = (message: string) => Promise<void>

/** 把任意参数转成可读字符串：Error 取 stack/message，对象走 JSON，其余 String 化。 */
function format(arg: unknown): string {
  if (typeof arg === 'string') return arg
  if (arg instanceof Error) return arg.stack ?? `${arg.name}: ${arg.message}`
  try {
    return JSON.stringify(arg)
  } catch {
    return String(arg)
  }
}

/** 用 plugin-log 的 logger 包裹原生 console 方法：先原样输出，再转发到日志文件。 */
function forward(method: ConsoleMethod, logger: Logger): void {
  const original = console[method].bind(console)
  console[method] = (...args: unknown[]) => {
    original(...args)
    // 转发失败（例如非 Tauri 环境）静默忽略，绝不影响业务
    void logger(args.map(format).join(' ')).catch(() => {})
  }
}

/** 安装 console -> 日志文件 的转发。幂等性由调用方保证（仅在 main.ts 调一次）。 */
export function setupLogger(): void {
  forward('log', debug)
  forward('debug', debug)
  forward('info', info)
  forward('warn', warn)
  forward('error', error)
}
