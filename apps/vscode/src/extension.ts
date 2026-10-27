import * as vscode from "vscode";
import { readFile, writeFile, stat } from "node:fs/promises";
import { hostname, platform, release, arch, homedir } from "node:os";
import { basename, join } from "node:path";
import { VscodeShareInbox } from "./shareInbox";

export function activate(context: vscode.ExtensionContext): void {
  const displayName = vscode.l10n.t("ClipRoam");
  let panel: vscode.WebviewPanel | undefined;
  let editor = vscode.window.activeTextEditor;
  let ready: Promise<void>;
  const shares = new VscodeShareInbox(context.globalState);
  const saveTargets = new Map<string, { uri: vscode.Uri; single: boolean }>();

  function exportPath(value: unknown): { uri: vscode.Uri; data: Record<string, unknown> } {
    if (!value || typeof value !== "object") throw new Error("导出参数无效");
    const data = value as Record<string, unknown>;
    const target = typeof data.target === "string" ? saveTargets.get(data.target) : undefined;
    const parts = data.path;
    if (!target || !Array.isArray(parts) || !parts.length || parts.some((part) =>
      typeof part !== "string" || !part || part === "." || part === ".." || /[<>:"/\\|?*\x00-\x1f]/.test(part))) throw new Error("保存位置或文件路径无效");
    if (target.single && parts.length !== 1) throw new Error("单文件保存不支持子目录");
    return { uri: target.single ? target.uri : vscode.Uri.joinPath(target.uri, ...parts), data };
  }

  context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor((active) => {
    if (active) editor = active;
  }));

  async function open(): Promise<vscode.WebviewPanel> {
    if (panel) { panel.reveal(panel.viewColumn); await ready; return panel; }
    const resourceRoot = vscode.Uri.joinPath(context.extensionUri, "dist", "webview");
    const current = vscode.window.createWebviewPanel("cliproam.history", displayName, vscode.ViewColumn.Beside, {
      enableScripts: true, retainContextWhenHidden: true, localResourceRoots: [resourceRoot],
    });
    panel = current;
    let markReady!: () => void;
    let rejectReady!: (error: Error) => void;
    const readyTimeout = setTimeout(() => rejectReady(new Error("ClipRoam 界面加载超时，请关闭标签页后重试")), 30_000);
    ready = new Promise<void>((resolve, reject) => {
      markReady = () => { clearTimeout(readyTimeout); resolve(); };
      rejectReady = (error) => { clearTimeout(readyTimeout); reject(error); };
    });
    // Loading can fail before open() reaches its await.
    void ready.catch(() => undefined);
    const listener = current.webview.onDidReceiveMessage(async (message: unknown) => {
      if (!message || typeof message !== "object") return;
      const data = message as Record<string, unknown>;
      if (data.type === "ready") { markReady(); return; }
      if (data.type !== "request" || typeof data.id !== "number" || typeof data.operation !== "string") return;
      try {
        const value = await perform(data.operation, data.value);
        await current.webview.postMessage({ type: "response", id: data.id, value });
      } catch (error) {
        await current.webview.postMessage({ type: "response", id: data.id, error: error instanceof Error ? error.message : String(error) });
      }
    });
    current.onDidDispose(() => {
      listener.dispose();
      rejectReady(new Error("ClipRoam 标签页已关闭"));
      if (panel === current) panel = undefined;
      saveTargets.clear();
    }, undefined, context.subscriptions);
    try {
      const html = await readFile(vscode.Uri.joinPath(resourceRoot, "index.html").fsPath, "utf8");
      const source = current.webview.cspSource;
      const csp = `default-src 'none'; script-src ${source}; style-src ${source} 'unsafe-inline'; img-src ${source} data: blob: http: https:; media-src ${source} blob: http: https:; font-src ${source} data:; connect-src http: https: ws: wss:;`;
      current.webview.html = html.replace("<head>", `<head><meta http-equiv="Content-Security-Policy" content="${csp}">`)
        .replace(/(src|href)="\.\/([^"<>]+)"/g, (_match, attr: string, file: string) =>
          `${attr}="${current.webview.asWebviewUri(vscode.Uri.joinPath(resourceRoot, file))}"`);
      await ready;
      return current;
    } catch (error) { current.dispose(); throw error; }
  }

  function selectionText(): string {
    if (!editor || editor.document.isClosed) throw new Error("请先打开文本编辑器");
    const text = editor.selections.map((selection) => editor!.document.getText(selection)).filter(Boolean).join("\n");
    if (!text.trim()) throw new Error("请在编辑器中选中要保存的文本");
    return text;
  }

  async function perform(operation: string, value: unknown): Promise<unknown> {
    switch (operation) {
      case "clipboard.read": return vscode.env.clipboard.readText();
      case "clipboard.write":
        if (typeof value !== "string") throw new Error("剪贴板文本无效");
        return vscode.env.clipboard.writeText(value);
      case "files.selectSaveTarget": {
        if (!value || typeof value !== "object") throw new Error("保存参数无效");
        const { single, fileName } = value as Record<string, unknown>;
        if (typeof single !== "boolean" || typeof fileName !== "string" || !fileName || /[<>:"/\\|?*\x00-\x1f]/.test(fileName) || fileName === "." || fileName === "..") throw new Error("保存文件名无效");
        const uri = single
          ? await vscode.window.showSaveDialog({ defaultUri: vscode.Uri.file(join(homedir(), fileName)), saveLabel: "保存" })
          : (await vscode.window.showOpenDialog({ canSelectFiles: false, canSelectFolders: true, canSelectMany: false, openLabel: "保存到此文件夹" }))?.[0];
        if (!uri) return null;
        if (uri.scheme !== "file") throw new Error("请选择本机保存位置");
        saveTargets.set(uri.fsPath, { uri, single });
        return { kind: "path", value: uri.fsPath };
      }
      case "files.createDirectory": {
        const { uri, data } = exportPath(value);
        if (saveTargets.get(data.target as string)!.single) throw new Error("单文件保存不支持创建目录");
        await vscode.workspace.fs.createDirectory(uri);
        return;
      }
      case "files.write": {
        const { uri, data } = exportPath(value);
        if (typeof data.data !== "string" || typeof data.append !== "boolean") throw new Error("文件内容无效");
        await writeFile(uri.fsPath, Buffer.from(data.data, "base64"), { flag: data.append ? "a" : "w" });
        return;
      }
      case "files.release":
        if (typeof value !== "string") throw new Error("保存位置标识无效");
        saveTargets.delete(value);
        return;
      case "shares.pending": {
        if (typeof value !== "string" || !value) throw new Error("账号档案无效");
        return shares.pending(value);
      }
      case "shares.acknowledge":
        if (typeof value !== "string") throw new Error("待处理文本标识无效");
        return shares.acknowledge(value);
      case "shares.readFile": {
        if (!value || typeof value !== "object") throw new Error("文件参数无效");
        const { id, path } = value as Record<string, unknown>;
        if (typeof id !== "string" || typeof path !== "string" || !shares.hasFile(id, path)) throw new Error("待处理文件不存在");
        const info = await stat(path);
        if (!info.isFile()) throw new Error("请选择文件");
        return { name: basename(path), lastModified: info.mtimeMs, data: (await readFile(path)).toString("base64") };
      }
      case "device.info": return {
        systemLocale: vscode.env.language,
        deviceName: `${hostname()} (VS Code)`, osType: platform(), osVersion: release(), architecture: arch(),
      };
      case "external.open": {
        if (typeof value !== "string") throw new Error("链接无效");
        const uri = vscode.Uri.parse(value, true);
        if (uri.scheme !== "http" && uri.scheme !== "https") throw new Error("仅支持 HTTP/HTTPS 链接");
        return vscode.env.openExternal(uri);
      }
      default: throw new Error("不支持的 VS Code 操作");
    }
  }

  function command(name: string, action: (...args: unknown[]) => Promise<unknown>): void {
    context.subscriptions.push(vscode.commands.registerCommand(name, async (...args: unknown[]) => {
      try { await action(...args); }
      catch (error) { void vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error)); }
    }));
  }
  command("cliproam.open", open);
  command("cliproam.captureSelection", async () => {
    const text = selectionText();
    await shares.enqueue(text);
    const current = await open();
    await current.webview.postMessage({ type: "sharesChanged" });
    void vscode.window.showInformationMessage("选中文本已暂存，进入主界面后自动加入历史");
  });
  command("cliproam.captureFile", async (resource, selected) => {
    const resources = Array.isArray(selected) && selected.length ? selected : [resource ?? vscode.window.activeTextEditor?.document.uri];
    const paths: string[] = [];
    for (const uri of resources) {
      if (!(uri instanceof vscode.Uri) || uri.scheme !== "file") throw new Error("请选择本机文件");
      if (!(await stat(uri.fsPath)).isFile()) throw new Error("请选择文件，不支持文件夹");
      paths.push(uri.fsPath);
    }
    await shares.enqueueFiles([...new Set(paths)]);
    const current = await open();
    await current.webview.postMessage({ type: "sharesChanged" });
    void vscode.window.showInformationMessage("文件已暂存，进入主界面后自动加入历史");
  });
  context.subscriptions.push({ dispose: () => panel?.dispose() });
}
