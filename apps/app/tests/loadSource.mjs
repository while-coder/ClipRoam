import { readFileSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import * as protocol from "@cliproam/protocol";
const require = createRequire(import.meta.url);
const ts = require("typescript");
const root = fileURLToPath(new URL("../src/", import.meta.url));

/** Load the actual TS services with explicit platform substitutes, without bundling Vue UI. */
export function sourceLoader({ native = false, call = async () => { throw new Error("Unexpected native call"); } } = {}) {
  const cache = new Map();
  const runtime = { isNative: native, nativeCall: call, nativeListen: async () => () => {} };
  function load(path) {
    let file = resolve(root,path);
    if (!existsSync(file)) file += ".ts";
    if (file === resolve(root,"platform/runtime.ts")) return runtime;
    if (file === resolve(root,"features/toast/useToast.ts")) return { showToast: () => {} };
    if (cache.has(file)) return cache.get(file).exports;
    const module = { exports: {} }; cache.set(file,module);
    const compiled = ts.transpileModule(readFileSync(file,"utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
    }).outputText;
    new Function("require","module","exports",compiled)(
      (id) => id === "@cliproam/protocol" ? protocol : id.startsWith(".") ? load(resolve(dirname(file),id)) : require(id), module,module.exports,
    );
    return module.exports;
  }
  return load;
}
