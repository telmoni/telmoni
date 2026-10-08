import { Step, Steps } from "fumadocs-ui/components/steps";
import { Tab, Tabs } from "fumadocs-ui/components/tabs";
import defaultMdxComponents from "fumadocs-ui/mdx";
import type { MDXComponents } from "mdx/types";

// What a documentation page may use without importing it: Fumadocs' own —
// headings, links, code, callouts, cards — and the two the pages reach for.
export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return { ...defaultMdxComponents, Tabs, Tab, Steps, Step, ...components };
}

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
