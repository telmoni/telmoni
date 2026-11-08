import { findNeighbour } from "fumadocs-core/page-tree";
import { createRelativeLink } from "fumadocs-ui/mdx";
import type { Metadata } from "next";
import { notFound } from "next/navigation";

import { getMDXComponents } from "@/components/mdx";
import { REPO_URL } from "@/lib/site";
import { source } from "@/lib/source";

import { DocsPageView } from "../_docs-shell";

type Params = PageProps<"/docs/[[...slug]]">;

// One page of the book, by its path under `content/docs`; `/docs` itself is
// the index page.
export default async function DocsRoute({ params }: Params) {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();

  const MDX = page.data.body;
  const neighbours = findNeighbour(source.getPageTree(), page.url);

  return (
    <DocsPageView
      title={page.data.title}
      description={page.data.description}
      toc={page.data.toc}
      neighbours={neighbours}
      repoUrl={REPO_URL}
    >
      <MDX components={getMDXComponents({ a: createRelativeLink(source, page) })} />
    </DocsPageView>
  );
}

export async function generateMetadata({ params }: Params): Promise<Metadata> {
  const { slug } = await params;
  const page = source.getPage(slug);
  if (!page) notFound();
  return { title: page.data.title, description: page.data.description };
}
