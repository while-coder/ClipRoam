import { execFileSync } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import { resolve } from "node:path";

const ports = [];
let executablePath;

for (let index = 2; index < process.argv.length; index += 1) {
  const argument = process.argv[index];
  if (argument === "--port") {
    ports.push(Number.parseInt(process.argv[++index], 10));
  } else if (argument === "--executable") {
    executablePath = process.argv[++index];
  } else {
    throw new Error(`Unknown argument: ${argument}`);
  }
}

function commandOutput(command, args, options = {}) {
  try {
    return execFileSync(command, args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], ...options });
  } catch (error) {
    if (error.status === 1) {
      return "";
    }
    throw error;
  }
}

function listenerProcessIds(port) {
  if (process.platform === "win32") {
    const output = commandOutput("netstat", ["-ano", "-p", "TCP"]);
    return output
      .split(/\r?\n/)
      .map((line) => line.trim().match(/^TCP\s+\S+:(\d+)\s+\S+\s+LISTENING\s+(\d+)$/i))
      .filter((match) => match?.[1] === String(port))
      .map((match) => Number.parseInt(match[2], 10));
  }

  return commandOutput("lsof", ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"])
    .split(/\r?\n/)
    .filter(Boolean)
    .map((value) => Number.parseInt(value, 10));
}

function executableProcessIds(path) {
  const platformExecutablePath = process.platform === "win32" && !path.toLowerCase().endsWith(".exe")
    ? `${path}.exe`
    : path;
  const targetPath = existsSync(platformExecutablePath)
    ? realpathSync(platformExecutablePath)
    : resolve(platformExecutablePath);

  if (process.platform === "win32") {
    const output = commandOutput(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        "Get-CimInstance Win32_Process -Filter \"Name = 'cliproam.exe'\" | Select-Object ProcessId, ExecutablePath | ConvertTo-Json -Compress",
      ],
    );
    if (!output.trim()) return [];
    const processes = JSON.parse(output);
    return (Array.isArray(processes) ? processes : [processes])
      .filter((process) => process.ExecutablePath?.toLowerCase() === targetPath.toLowerCase())
      .map((process) => process.ProcessId);
  }

  return commandOutput("ps", ["-axo", "pid=,command="])
    .split(/\r?\n/)
    .map((line) => line.trim().match(/^(\d+)\s+(.+)$/))
    .filter((match) => match && (match[2] === targetPath || match[2].startsWith(`${targetPath} `)))
    .map((match) => Number.parseInt(match[1], 10));
}

function stopProcess(processId) {
  try {
    process.kill(processId, "SIGTERM");
    console.log(`Stopping PID ${processId}`);
  } catch (error) {
    if (error.code !== "ESRCH") {
      throw error;
    }
  }
}

async function waitForPortsToRelease() {
  if (!ports.length) return;
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const occupied = ports.filter((port) => listenerProcessIds(port).length > 0);
    if (!occupied.length) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const occupied = ports.filter((port) => listenerProcessIds(port).length > 0);
  throw new Error(`Timed out waiting for port(s) to close: ${occupied.join(", ")}`);
}

const processIds = new Set(ports.flatMap(listenerProcessIds));
if (executablePath) {
  executableProcessIds(executablePath).forEach((processId) => processIds.add(processId));
}
processIds.forEach(stopProcess);
await waitForPortsToRelease();
