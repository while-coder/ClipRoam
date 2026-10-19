import { Marked } from "marked";
import DOMPurify from "dompurify";
import { highlightText } from "./textHighlight";

const markdown = new Marked({
  gfm: true,
  renderer: {
    code({ text, lang }) {
      const language = lang?.trim().split(/\s+/)[0] || "auto";
      return `<pre><code class="hljs">${highlightText(text, language).html}</code></pre>\n`;
    },
  },
});

export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(markdown.parse(text, { async: false }), {
    USE_PROFILES: { html: true },
    FORBID_TAGS: ["style", "form"],
    FORBID_ATTR: ["style"],
  });
}
