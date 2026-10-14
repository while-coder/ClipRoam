import * as vscode from "vscode";
import { readFile } from "node:fs/promises";
import { hostname, platform, release, arch } from "node:os";

export function activate(context: vscode.ExtensionContext): void {
  let panel: vscode.WebviewPanel | undefined;
  let editor = vscode.window.activeTextEditor;
  let ready: Promise<void>;
  let nextId = 0;
  const captures = new Map<number, { resolve(): void; reject(error: Error): void; timer: ReturnType<typeof setTimeout> }>();

  context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor((active) => {
    if (active) editor = active;
  }));

  async function open(): Promise<vscode.WebviewPanel> {
    if (panel) { panel.reveal(panel.viewColumn); await ready; return panel; }
    const resourceRoot = vscode.Uri.joinPath(context.extensionUri, "dist", "webview");
    const current = vscode.window.createWebviewPanel("cliproam.history", "ClipRoam", vscode.ViewColumn.Beside, {
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
      if (data.type === "captured" && typeof data.id === "number") {
        const capture = captures.get(data.id);
        if (!capture) return;
        captures.delete(data.id); clearTimeout(capture.timer);
        if (typeof data.error === "string") capture.reject(new Error(data.error));
        else capture.resolve();
        return;
      }
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
      for (const capture of captures.values()) {
        clearTimeout(capture.timer); capture.reject(new Error("ClipRoam 标签页已关闭"));
      }
      captures.clear();
    }, undefined, context.subscriptions);
    try {
      const html = await readFile(vscode.Uri.joinPath(resourceRoot, "index.html").fsPath, "utf8");
      const source = current.webview.cspSource;
      const csp = `default-src 'none'; script-src ${source}; style-src ${source} 'unsafe-inline'; img-src ${source} data: blob: http: https:; font-src ${source} data:; connect-src http: https: ws: wss:;`;
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
      case "editor.selection": return selectionText();
      case "editor.insert": {
        if (typeof value !== "string") throw new Error("插入文本无效");
        const target = editor;
        if (!target || target.document.isClosed) throw new Error("请先打开要插入文本的编辑器");
        const selections = [...target.selections];
        const active = await vscode.window.showTextDocument(target.document, target.viewColumn);
        const inserted = await active.edit((edit) => { for (const selection of selections) edit.replace(selection, value); });
        if (!inserted) throw new Error("无法插入文本，请检查文件是否只读");
        return;
      }
      case "device.info": return {
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

  function command(name: string, action: () => Promise<unknown>): void {
    context.subscriptions.push(vscode.commands.registerCommand(name, async () => {
      try { await action(); }
      catch (error) { void vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error)); }
    }));
  }
  command("cliproam.open", open);
  command("cliproam.insertHistory", async () => { await (await open()).webview.postMessage({ type: "insertHistory" }); });
  command("cliproam.captureSelection", async () => {
    const text = selectionText();
    const current = await open();
    const id = ++nextId;
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => { captures.delete(id); reject(new Error("保存选中文本超时，请重试")); }, 30_000);
      captures.set(id, { resolve, reject, timer });
      void current.webview.postMessage({ type: "captureSelection", id, text }).then((sent) => {
        if (!sent) { clearTimeout(timer); captures.delete(id); reject(new Error("ClipRoam 界面不可用")); }
      });
    });
    void vscode.window.showInformationMessage("已加入 ClipRoam 上传队列");
  });
  context.subscriptions.push({ dispose: () => panel?.dispose() });
}
