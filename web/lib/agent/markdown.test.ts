import { describe, expect, it } from "vitest";

import {
  allowedHref,
  opensInConsole,
  parseInline,
  parseMarkdown,
  type Block,
  type Inline,
} from "./markdown";

function textOf(nodes: Inline[]): string {
  return nodes
    .map((n) => {
      switch (n.type) {
        case "text":
        case "code":
          return n.text;
        case "citation":
          return `[${n.index}]`;
        default:
          return textOf(n.children);
      }
    })
    .join("");
}

function links(blocks: Block[]): Inline[] {
  const found: Inline[] = [];
  const walk = (nodes: Inline[]) => {
    for (const n of nodes) {
      if (n.type === "link") found.push(n);
      if ("children" in n) walk(n.children);
    }
  };
  for (const b of blocks) {
    if (b.type === "paragraph" || b.type === "heading") walk(b.children);
    if (b.type === "list") b.items.forEach(walk);
  }
  return found;
}

describe("parseMarkdown", () => {
  it("splits paragraphs on blank lines and keeps a soft break inside one", () => {
    const blocks = parseMarkdown("First line\nsecond line\n\nNext paragraph");
    expect(blocks).toEqual([
      { type: "paragraph", children: [{ type: "text", text: "First line\nsecond line" }] },
      { type: "paragraph", children: [{ type: "text", text: "Next paragraph" }] },
    ]);
  });

  it("reads a heading as its own block", () => {
    expect(parseMarkdown("## Members")).toEqual([
      { type: "heading", children: [{ type: "text", text: "Members" }] },
    ]);
  });

  it("reads bulleted and numbered lists", () => {
    const blocks = parseMarkdown("- one\n- two\n\n3. three\n4. four");
    expect(blocks).toHaveLength(2);
    expect(blocks[0]).toMatchObject({ type: "list", ordered: false, start: 1 });
    expect(blocks[1]).toMatchObject({ type: "list", ordered: true, start: 3 });
    const [first, second] = blocks as Extract<Block, { type: "list" }>[];
    expect(first!.items.map(textOf)).toEqual(["one", "two"]);
    expect(second!.items.map(textOf)).toEqual(["three", "four"]);
  });

  it("folds a nested item into the one above it", () => {
    const [list] = parseMarkdown("- parent\n  - child") as Extract<Block, { type: "list" }>[];
    expect(list!.items.map(textOf)).toEqual(["parent child"]);
  });

  it("keeps a fenced block verbatim, markup and all", () => {
    const blocks = parseMarkdown("```bash\ncurl **not bold** <b>\n```\nafter");
    expect(blocks[0]).toEqual({ type: "code", lang: "bash", text: "curl **not bold** <b>" });
    expect(blocks[1]).toEqual({ type: "paragraph", children: [{ type: "text", text: "after" }] });
  });

  it("runs an unclosed fence to the end, as a reply mid-stream has one", () => {
    expect(parseMarkdown("```\npartial")).toEqual([
      { type: "code", lang: null, text: "partial" },
    ]);
  });

  it("keeps raw HTML as text", () => {
    const blocks = parseMarkdown('<script>alert("x")</script> <img src=x onerror=alert(1)>');
    expect(blocks).toEqual([
      {
        type: "paragraph",
        children: [
          { type: "text", text: '<script>alert("x")</script> <img src=x onerror=alert(1)>' },
        ],
      },
    ]);
  });
});

describe("parseInline", () => {
  it("reads inline code, bold and italic", () => {
    expect(parseInline("run `make up` **now**, *please*")).toEqual([
      { type: "text", text: "run " },
      { type: "code", text: "make up" },
      { type: "text", text: " " },
      { type: "strong", children: [{ type: "text", text: "now" }] },
      { type: "text", text: ", " },
      { type: "em", children: [{ type: "text", text: "please" }] },
    ]);
  });

  it("leaves a snake_case name alone", () => {
    expect(parseInline("call list_members and audit_events")).toEqual([
      { type: "text", text: "call list_members and audit_events" },
    ]);
  });

  it("reads a citation marker", () => {
    expect(parseInline("Two members [1].")).toEqual([
      { type: "text", text: "Two members " },
      { type: "citation", index: 1 },
      { type: "text", text: "." },
    ]);
  });

  it("keeps a console path as a link", () => {
    expect(parseInline("[Members](/project_abc/members)")).toEqual([
      {
        type: "link",
        href: "/project_abc/members",
        children: [{ type: "text", text: "Members" }],
      },
    ]);
  });

  // The docs are the console's own pages, cited by path in the agent's corpus.
  it("keeps a link into the docs as a console link", () => {
    expect(parseInline("[webhooks](/docs/integrations/webhooks#verifying-signatures)")).toEqual([
      {
        type: "link",
        href: "/docs/integrations/webhooks#verifying-signatures",
        children: [{ type: "text", text: "webhooks" }],
      },
    ]);
  });

  it("renders an injected javascript: link as its own text", () => {
    const nodes = parseInline("[click me](javascript:alert(document.cookie))");
    expect(nodes.every((n) => n.type === "text")).toBe(true);
    expect(textOf(nodes)).toBe("[click me](javascript:alert(document.cookie))");
  });

  it("never turns an image into anything but its text", () => {
    const source = "![x](https://evil.example/?q=secret)";
    const blocks = parseMarkdown(source);
    expect(blocks).toEqual([{ type: "paragraph", children: [{ type: "text", text: source }] }]);
    expect(links(blocks)).toEqual([]);
  });

  it("does not let an image on a console path through either", () => {
    const source = "![x](/docs/logo.png)";
    expect(parseInline(source)).toEqual([{ type: "text", text: source }]);
  });

  it.each([
    "//evil.example/path",
    "/\\evil.example",
    "https://telmoni.com/docs/x",
    "https://evil.example/",
    "data:text/html,<b>hi</b>",
    "javascript:alert(1)",
  ])("renders a link to %s as text", (href) => {
    const blocks = parseMarkdown(`see [here](${href})`);
    expect(links(blocks)).toEqual([]);
  });
});

describe("allowedHref", () => {
  it("allows a console path, the docs' included, and nothing else", () => {
    expect(allowedHref("/acme/audit-log")).toBe("/acme/audit-log");
    expect(allowedHref("/docs/integrations/webhooks#verifying-signatures")).toBe(
      "/docs/integrations/webhooks#verifying-signatures",
    );
    expect(allowedHref("https://telmoni.com/docs")).toBeNull();
    expect(allowedHref("//telmoni.com/docs")).toBeNull();
    expect(allowedHref("/ok path")).toBeNull();
    expect(allowedHref("")).toBeNull();
  });
});

describe("opensInConsole", () => {
  it("keeps the console's pages in place and sends the docs to a new tab", () => {
    expect(opensInConsole("/acme/web/members")).toBe(true);
    expect(opensInConsole("/acme/audit-log?page=2")).toBe(true);
    expect(opensInConsole("/acme#top")).toBe(true);
    expect(opensInConsole("/account/privacy")).toBe(true);
    expect(opensInConsole("/docs/integrations/webhooks#verifying-signatures")).toBe(false);
    expect(opensInConsole("/docs")).toBe(false);
  });
});
