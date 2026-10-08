/**
 * The agent's replies, parsed into nodes the panel maps to React elements.
 *
 * ⚠ **Nothing here produces HTML, and nothing it produces is fetched.** A reply
 * is model output, and a model can be steered by what it reads — an audit
 * event's free text, a delivery's error — into writing whatever an attacker
 * put there. So `<script>` stays text (React escapes it), an image is its own
 * literal source rather than an `<img>` whose URL would carry a query string
 * out the moment it rendered, and a link survives only when it points into
 * the console, its docs included. Everything else is text a person can read and
 * choose not to follow.
 */

export type Inline =
  | { type: "text"; text: string }
  | { type: "code"; text: string }
  | { type: "strong"; children: Inline[] }
  | { type: "em"; children: Inline[] }
  | { type: "link"; href: string; children: Inline[] }
  | { type: "citation"; index: number };

export type Block =
  | { type: "paragraph"; children: Inline[] }
  | { type: "heading"; children: Inline[] }
  | { type: "list"; ordered: boolean; start: number; items: Inline[][] }
  | { type: "code"; lang: string | null; text: string };

// `null` when the href may not be a link: only a console path is one, the
// docs' paths included, since the agent's corpus cites them as paths. A path
// is same-origin only when it cannot be read as a host: `//evil.example` is
// protocol-relative, and browsers read `\` as `/`, so `/\evil.example` is too.
export function allowedHref(raw: string): string | null {
  const href = raw.trim();
  if (href === "" || /[\s\\\u0000-\u001f\u007f]/.test(href)) return null;
  if (href.startsWith("/") && !href.startsWith("//")) return href;
  return null;
}

const FENCE = /^\s*(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/;
const HEADING = /^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$/;
const BULLET = /^\s{0,3}[-*+]\s+(.*)$/;
const ORDERED = /^\s{0,3}(\d{1,9})[.)]\s+(.*)$/;

function indentOf(line: string): number {
  return /^\s*/.exec(line)![0].length;
}

export function parseMarkdown(source: string): Block[] {
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let paragraph: string[] = [];

  const flush = () => {
    if (paragraph.length === 0) return;
    blocks.push({ type: "paragraph", children: parseInline(paragraph.join("\n")) });
    paragraph = [];
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;

    const fence = FENCE.exec(line);
    if (fence) {
      flush();
      const marker = fence[1]!;
      const body: string[] = [];
      i++;
      // An unclosed fence runs to the end, which is what a reply cut off
      // mid-block looks like while it is still streaming.
      while (i < lines.length && !lines[i]!.trim().startsWith(marker)) {
        body.push(lines[i]!);
        i++;
      }
      i++;
      blocks.push({ type: "code", lang: fence[2] || null, text: body.join("\n") });
      continue;
    }

    if (line.trim() === "") {
      flush();
      i++;
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      flush();
      blocks.push({ type: "heading", children: parseInline(heading[1]!) });
      i++;
      continue;
    }

    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (bullet || ordered) {
      flush();
      const isOrdered = !bullet;
      const start = ordered ? Number(ordered[1]) : 1;
      const baseIndent = indentOf(line);
      const items: string[] = [];
      while (i < lines.length) {
        const current = lines[i]!;
        // A nested item or a wrapped line joins the item above it: the panel
        // draws one level, and a lost line would be worse than a flat one.
        if (
          items.length > 0 &&
          current.trim() !== "" &&
          indentOf(current) > baseIndent &&
          !FENCE.test(current)
        ) {
          const nested = BULLET.exec(current.trim()) ?? ORDERED.exec(current.trim());
          const text = nested ? nested[nested.length - 1]! : current.trim();
          items[items.length - 1] = `${items[items.length - 1]} ${text}`;
          i++;
          continue;
        }
        const item = isOrdered ? ORDERED.exec(current)?.[2] : BULLET.exec(current)?.[1];
        if (item === undefined) break;
        items.push(item);
        i++;
      }
      blocks.push({
        type: "list",
        ordered: isOrdered,
        start,
        items: items.map((t) => parseInline(t)),
      });
      continue;
    }

    paragraph.push(line);
    i++;
  }
  flush();
  return blocks;
}

const IMAGE = /^!\[[^\]]*\]\([^)]*\)/;
const LINK = /^\[([^\]]+)\]\(\s*([^)\s]*)(?:\s+"[^"]*")?\s*\)/;
const CITATION = /^\[(\d{1,4})\]/;

function isWordChar(ch: string | undefined): boolean {
  return ch !== undefined && /[\p{L}\p{N}]/u.test(ch);
}

export function parseInline(source: string, inLink = false): Inline[] {
  const out: Inline[] = [];
  let text = "";
  const pushText = (t: string) => {
    text += t;
  };
  const pushNode = (node: Inline) => {
    if (text) out.push({ type: "text", text });
    text = "";
    out.push(node);
  };

  let i = 0;
  while (i < source.length) {
    const rest = source.slice(i);
    const ch = source[i]!;

    if (ch === "`") {
      const run = /^`+/.exec(rest)![0];
      const close = source.indexOf(run, i + run.length);
      if (close !== -1) {
        pushNode({ type: "code", text: source.slice(i + run.length, close).trim() });
        i = close + run.length;
        continue;
      }
      pushText(run);
      i += run.length;
      continue;
    }

    if (ch === "!") {
      const image = IMAGE.exec(rest);
      if (image) {
        pushText(image[0]);
        i += image[0].length;
        continue;
      }
    }

    if (ch === "[") {
      const link = LINK.exec(rest);
      if (link && !inLink) {
        const href = allowedHref(link[2]!);
        if (href) {
          pushNode({ type: "link", href, children: parseInline(link[1]!, true) });
        } else {
          pushText(link[0]);
        }
        i += link[0].length;
        continue;
      }
      const citation = CITATION.exec(rest);
      if (citation && source[i + citation[0].length] !== "(") {
        pushNode({ type: "citation", index: Number(citation[1]) });
        i += citation[0].length;
        continue;
      }
    }

    if ((ch === "*" || ch === "_") && source[i + 1] === ch) {
      const marker = ch + ch;
      const close = source.indexOf(marker, i + 2);
      if (close > i + 2) {
        pushNode({ type: "strong", children: parseInline(source.slice(i + 2, close), inLink) });
        i = close + 2;
        continue;
      }
    }

    // `_` only at a word's edge: tool and field names are snake_case, and
    // `list_members` italicising its middle would misprint the very names a
    // reply about this product is full of.
    if (ch === "*" || (ch === "_" && !isWordChar(source[i - 1]))) {
      let close = source.indexOf(ch, i + 1);
      while (ch === "_" && close !== -1 && isWordChar(source[close + 1])) {
        close = source.indexOf(ch, close + 1);
      }
      if (close > i + 1 && source[i + 1] !== " ") {
        pushNode({ type: "em", children: parseInline(source.slice(i + 1, close), inLink) });
        i = close + 1;
        continue;
      }
    }

    pushText(ch);
    i++;
  }
  if (text) out.push({ type: "text", text });
  return out;
}
