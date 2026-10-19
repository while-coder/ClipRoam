export const textLanguages = [
  ["json", "JSON"], ["xml", "HTML / XML"], ["yaml", "YAML"], ["ini", "INI"],
  ["javascript", "JavaScript"], ["typescript", "TypeScript"], ["css", "CSS"],
  ["python", "Python"], ["csharp", "C#"], ["cpp", "C / C++"],
  ["java", "Java"], ["go", "Go"], ["rust", "Rust"], ["bash", "Shell"],
  ["sql", "SQL"], ["diff", "Diff"], ["markdown", "Markdown"],
] as const;

const extensions: Record<string, string> = {
  json: "json", xml: "xml", html: "xml", htm: "xml", svg: "xml", vue: "xml",
  yaml: "yaml", yml: "yaml", ini: "ini", conf: "ini", cfg: "ini",
  js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  ts: "typescript", tsx: "typescript", css: "css", py: "python", cs: "csharp",
  c: "cpp", h: "cpp", cc: "cpp", cpp: "cpp", hpp: "cpp", java: "java",
  go: "go", rs: "rust", sh: "bash", bash: "bash", sql: "sql", diff: "diff", patch: "diff",
  md: "markdown", markdown: "markdown", txt: "plaintext", log: "plaintext",
  csv: "plaintext", tsv: "plaintext",
};

export function languageForFile(fileName?: string): string | undefined {
  if (!fileName?.includes(".")) return undefined;
  return extensions[fileName.split(".").pop()!.toLowerCase()];
}

// Bound synchronous parsing work; large previews remain readable as plain text.
export const highlightLimit = 100_000;
