#!/usr/bin/env node

// 释放被占用的端口:找到监听该端口的进程,连同启动它的工具链父进程一起结束。
// 用法: node kill-port.js <端口>
//
// 为什么不直接杀监听进程:`tsx --watch` 这类父进程会在子进程被杀后重新拉起,
// 因此必须沿祖先链把整个工具链一起结束(遇到 shell 终端即停止,不会关闭终端)。

const { execFileSync } = require("node:child_process");

const port = Number(process.argv[2]);
if (!Number.isInteger(port) || port <= 0) {
  console.error("用法: node kill-port.js <端口>");
  process.exit(1);
}

function findListenerPids() {
  if (process.platform === "win32") {
    const output = execFileSync("netstat", ["-ano"], { encoding: "utf8" });
    const pids = new Set();
    for (const line of output.split("\n")) {
      const columns = line.trim().split(/\s+/);
      // 协议 本地地址 远程地址 状态 PID
      if (columns.length === 5 && columns[3] === "LISTENING" && columns[1].endsWith(`:${port}`)) {
        pids.add(Number(columns[4]));
      }
    }
    return [...pids];
  }
  const output = execFileSync("lsof", [`-ti:${port}`, "-sTCP:LISTEN"], { encoding: "utf8" });
  return [...new Set(output.split("\n").map(Number).filter(Boolean))];
}

const TOOLCHAIN_NAMES = new Set(["node", "node.exe", "pnpm", "npm", "npx", "cross-env", "tsx", "shx"]);
const TOOLCHAIN_CMD = /pnpm|tsx|cross-env/;

function isToolchainProcess(name, commandLine) {
  if (TOOLCHAIN_NAMES.has(name)) return true;
  // Windows 上 pnpm 会包一层 cmd.exe /c pnpm ...
  return name === "cmd.exe" && TOOLCHAIN_CMD.test(commandLine);
}

function findWinProcess(procId) {
  const script = `Get-CimInstance Win32_Process -Filter "ProcessId=${procId}" | Select-Object -First 1 | ForEach-Object { "$($_.ParentProcessId)|$($_.Name)|$($_.CommandLine)" }`;
  try {
    const output = execFileSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" }).trim();
    if (!output) return null;
    const separator = output.indexOf("|");
    const parent = Number(output.slice(0, separator));
    const rest = output.slice(separator + 1);
    const nameSeparator = rest.indexOf("|");
    return { parent, name: rest.slice(0, nameSeparator), commandLine: rest.slice(nameSeparator + 1) };
  } catch {
    return null;
  }
}

function collectAncestorTree(pid) {
  const pids = [pid];
  let current = pid;
  for (let depth = 0; depth < 15; depth += 1) {
    if (process.platform === "win32") {
      const info = findWinProcess(current);
      if (!info || !info.parent || info.parent <= 4) break;
      const parent = findWinProcess(info.parent);
      if (!parent || !isToolchainProcess(parent.name, parent.commandLine)) break;
      pids.push(info.parent);
      current = info.parent;
    } else {
      let parentPid;
      let parentName;
      try {
        parentPid = Number(execFileSync("ps", ["-o", "ppid=", "-p", String(current)], { encoding: "utf8" }).trim());
        parentName = execFileSync("ps", ["-o", "comm=", "-p", String(parentPid)], { encoding: "utf8" }).trim();
      } catch {
        break;
      }
      if (!parentPid || parentPid <= 1) break;
      if (!isToolchainProcess(parentName.split("/").pop(), "")) break;
      pids.push(parentPid);
      current = parentPid;
    }
  }
  return pids;
}

function killProcess(target) {
  try {
    if (process.platform === "win32") {
      execFileSync("taskkill", ["/PID", String(target), "/F"], { stdio: "ignore" });
    } else {
      process.kill(target, "SIGKILL");
    }
  } catch {
    // 进程可能已经退出,忽略
  }
}

const pids = findListenerPids();
if (pids.length === 0) {
  console.log(`端口 ${port} 已空闲`);
  process.exit(0);
}
for (const pid of pids) {
  const tree = collectAncestorTree(pid);
  for (const target of tree) {
    killProcess(target);
  }
  console.log(`已结束进程: ${tree.join(" <- ")}`);
}
console.log(`已释放端口 ${port}`);
