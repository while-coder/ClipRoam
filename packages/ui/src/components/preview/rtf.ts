import DOMPurify from "dompurify";

export interface RtfAttachment { marker: string; name: string; }

/** Keep bytes intact: RTF code pages and hex escapes must be decoded by the parser. */
export function prepareRtf(bytes: Uint8Array): { bytes: Uint8Array; attachments: RtfAttachment[] } {
  if (bytes.length > 5 * 1024 * 1024) throw new Error("富文本正文超过 5 MiB，请下载后查看");
  let input = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    input += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  if (!/^\{\\rtf\d/.test(input)) throw new Error("文件不是有效的 RTF 正文；暂不支持扁平化 RTFD，请使用包含 TXT.rtf 的目录包");
  const attachments: RtfAttachment[] = [];
  // A random prefix prevents document text from impersonating an attachment marker.
  const prefix = `CLIPROAMATTACH${crypto.randomUUID().replace(/-/g, "")}X`;
  let output = "";
  let copied = 0;
  for (let index = 0; index < input.length; index++) {
    if (input[index] === "\\") {
      const binary = /^\\bin(\d+) ?/.exec(input.slice(index, index + 32));
      if (binary) index += binary[0].length + Number(binary[1]) - 1;
      else if (/[\\{}]/.test(input[index + 1] ?? "")) index++;
      continue;
    }
    if (input[index] !== "{") continue;
    const start = /^\{\s*\\(?:\*\s*\\)?NeXTGraphic\s+/.exec(input.slice(index, index + 64));
    if (!start) continue;
    let end = index + start[0].length;
    let depth = 1;
    let name = "";
    for (; end < input.length; end++) {
      const char = input[end]!;
      if (char === "\\") {
        const escape = /^\\'([\da-f]{2})/i.exec(input.slice(end, end + 4));
        if (escape) { name += String.fromCharCode(parseInt(escape[1]!, 16)); end += 3; }
        else if (/[\\{}]/.test(input[end + 1] ?? "")) { name += input[++end]; }
        else {
          const control = /^\\([a-z]+)(-?\d+)? ?/i.exec(input.slice(end));
          if (!control) throw new Error("RTFD 附件引用格式无效");
          if (control[1] === "bin") end += Number(control[2] ?? 0);
          end += control[0].length - 1;
        }
      } else if (char === "{") depth++;
      else if (char === "}" && --depth === 0) break;
      else if (depth === 1 && char !== "\r" && char !== "\n") name += char;
    }
    if (depth !== 0) throw new Error("RTFD 附件引用不完整");
    if (attachments.length >= 100) throw new Error("富文本附件超过 100 个，请下载后查看");
    const raw = Uint8Array.from(name.trim(), char => char.charCodeAt(0));
    let decoded: string;
    try { decoded = new TextDecoder("utf-8", { fatal: true }).decode(raw); }
    catch { decoded = new TextDecoder("windows-1252").decode(raw); }
    const marker = `${prefix}${attachments.length}END`;
    attachments.push({ marker, name: decoded });
    output += input.slice(copied, index) + marker;
    copied = end + 1;
    index = end;
  }
  output += input.slice(copied);
  return { bytes: Uint8Array.from(output, char => char.charCodeAt(0)), attachments };
}

/** Only resolve package-relative names; never fetch URLs or escape the package. */
export function attachmentPath(name: string): string | undefined {
  const path = name.replace(/\\/g, "/");
  if (!path || /^(?:[a-z][a-z\d+.-]*:|\/)/i.test(path) || path.includes("\0")) return;
  if (path.split("/").some(part => !part || part === "." || part === "..")) return;
  return path;
}

export async function renderRtf(bytes: Uint8Array): Promise<HTMLElement> {
  const { RTFJS: imported } = await import("rtf.js");
  const RTFJS = ("default" in imported ? imported.default : imported) as typeof imported;
  RTFJS.loggingEnabled(false);
  // IMPORT is disabled unless an onImport handler is provided; never supply one.
  const document = new RTFJS.Document(bytes.slice().buffer, {});
  const elements = await document.render();
  const container = window.document.createElement("div");
  container.append(...elements);
  // Preserve formatting emitted by the renderer, but no active or external content.
  const fragment = DOMPurify.sanitize(container.innerHTML, {
    RETURN_DOM_FRAGMENT: true,
    ALLOWED_TAGS: ["div", "span", "p", "br", "b", "strong", "i", "em", "u", "s", "sub", "sup", "table", "tbody", "tr", "td", "th", "img"],
    ALLOWED_ATTR: ["style", "src", "alt", "width", "height", "colspan", "rowspan"],
  });
  container.replaceChildren(fragment);
  for (const element of container.querySelectorAll<HTMLElement>("[style]")) {
    // Keep only text and paragraph presentation; discard URLs, positioning and layout overlays.
    const source = element.style;
    const safe = window.document.createElement("span").style;
    for (const property of ["font-family", "font-size", "font-weight", "font-style", "color", "background-color", "text-decoration", "text-align", "vertical-align", "white-space", "margin-left", "margin-right", "text-indent"]) {
      const value = source.getPropertyValue(property);
      if (value && !/url\s*\(|expression\s*\(|var\s*\(/i.test(value)) safe.setProperty(property, value);
    }
    element.setAttribute("style", safe.cssText);
  }
  for (const image of container.querySelectorAll("img")) {
    if (!/^data:image\/(?:png|jpeg|gif|bmp|webp);base64,/i.test(image.getAttribute("src") ?? "")) {
      image.replaceWith(window.document.createTextNode("[不支持的内嵌图片]"));
    }
  }
  return container;
}
