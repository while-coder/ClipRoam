import hljs from "highlight.js/lib/core";
import json from "highlight.js/lib/languages/json";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import ini from "highlight.js/lib/languages/ini";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import css from "highlight.js/lib/languages/css";
import python from "highlight.js/lib/languages/python";
import csharp from "highlight.js/lib/languages/csharp";
import cpp from "highlight.js/lib/languages/cpp";
import java from "highlight.js/lib/languages/java";
import go from "highlight.js/lib/languages/go";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";
import sql from "highlight.js/lib/languages/sql";
import diff from "highlight.js/lib/languages/diff";
import markdown from "highlight.js/lib/languages/markdown";
import "highlight.js/styles/github-dark.css";
import { highlightLimit } from "./textLanguages";

for (const [name, grammar] of Object.entries({ json, xml, yaml, ini, javascript, typescript,
  css, python, csharp, cpp, java, go, rust, bash, sql, diff, markdown })) {
  hljs.registerLanguage(name, grammar);
}

export function escapeText(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function highlightText(text: string, language = "auto"): { html: string; language: string } {
  const plain = () => ({ html: escapeText(text), language: "plaintext" });
  if (!text || language === "plaintext" || text.length > highlightLimit) return plain();
  try {
    if (language === "auto") {
      // Detect on a small sample, then highlight once using the selected grammar.
      const result = hljs.highlightAuto(text.slice(0, 8000), hljs.listLanguages());
      if (!result.language || result.relevance < 5) return plain();
      language = result.language;
    }
    if (!hljs.getLanguage(language)) return plain();
    return { html: hljs.highlight(text, { language, ignoreIllegals: true }).value, language };
  } catch {
    return plain();
  }
}
