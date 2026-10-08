"use client";

import { Fragment, useMemo } from "react";
import Link from "next/link";
import { allowedHref, parseMarkdown, type Block, type Inline } from "@/lib/agent/markdown";
import type { AgentCitation } from "@/lib/types/agent";

export function Reply({ content, citations }: { content: string; citations: AgentCitation[] }) {
  // Every streamed delta re-renders the whole log; only the reply it grew
  // needs parsing again.
  const blocks = useMemo(() => parseMarkdown(content), [content]);
  return (
    <div className="grid gap-2 text-sm wrap-break-word">
      {blocks.map((b, i) => (
        <BlockView key={i} block={b} citations={citations} />
      ))}
    </div>
  );
}

function BlockView({ block, citations }: { block: Block; citations: AgentCitation[] }) {
  switch (block.type) {
    case "paragraph":
      return (
        <p className="whitespace-pre-wrap">
          <Inlines nodes={block.children} citations={citations} />
        </p>
      );
    case "heading":
      return (
        <p className="font-semibold">
          <Inlines nodes={block.children} citations={citations} />
        </p>
      );
    case "list": {
      const items = block.items.map((item, i) => (
        <li key={i}>
          <Inlines nodes={item} citations={citations} />
        </li>
      ));
      return block.ordered ? (
        <ol start={block.start} className="grid list-decimal gap-1 pl-5">
          {items}
        </ol>
      ) : (
        <ul className="grid list-disc gap-1 pl-5">{items}</ul>
      );
    }
    case "code":
      return (
        <pre className="overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-xs">
          <code>{block.text}</code>
        </pre>
      );
  }
}

function Inlines({ nodes, citations }: { nodes: Inline[]; citations: AgentCitation[] }) {
  return (
    <>
      {nodes.map((n, i) => (
        <Fragment key={i}>
          <InlineView node={n} citations={citations} />
        </Fragment>
      ))}
    </>
  );
}

function InlineView({ node, citations }: { node: Inline; citations: AgentCitation[] }) {
  switch (node.type) {
    case "text":
      return node.text;
    case "code":
      return <code className="rounded-xs bg-muted px-1 py-0.5 font-mono text-xs">{node.text}</code>;
    case "strong":
      return (
        <strong className="font-semibold">
          <Inlines nodes={node.children} citations={citations} />
        </strong>
      );
    case "em":
      return (
        <em>
          <Inlines nodes={node.children} citations={citations} />
        </em>
      );
    case "link":
      return (
        <SafeLink href={node.href}>
          <Inlines nodes={node.children} citations={citations} />
        </SafeLink>
      );
    case "citation": {
      const source = citations.find((c) => c.index === node.index);
      const target = source?.url ? allowedHref(source.url) : null;
      if (!source || !target) return `[${node.index}]`;
      return (
        <sup>
          <SafeLink href={target} label={`Source ${node.index}: ${source.title}`}>
            [{node.index}]
          </SafeLink>
        </sup>
      );
    }
  }
}

// Only ever handed an href allowedHref passed: a console path, so it is a
// client navigation, and the panel stays open over the page it links to.
export function SafeLink({
  href,
  label,
  children,
}: {
  href: string;
  label?: string;
  children: React.ReactNode;
}) {
  return (
    <Link
      href={href}
      aria-label={label}
      className="text-primary underline underline-offset-2 hover:no-underline"
    >
      {children}
    </Link>
  );
}

export function Sources({ citations }: { citations: AgentCitation[] }) {
  const sorted = [...citations].sort((a, b) => a.index - b.index);
  return (
    <div className="grid gap-1 border-t border-border pt-2">
      <p className="text-xs font-medium tracking-wide text-muted-foreground uppercase">Sources</p>
      <ol className="grid gap-1 text-xs">
        {sorted.map((c) => {
          const target = c.url ? allowedHref(c.url) : null;
          return (
            <li key={c.index} className="flex gap-1.5">
              <span className="shrink-0 text-muted-foreground">[{c.index}]</span>
              {target ? (
                <SafeLink href={target}>
                  {c.title}
                </SafeLink>
              ) : (
                <span>{c.title}</span>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
