import { llms, loader } from "fumadocs-core/source";
import { defineDocs } from "fumadocs-mdx/macro";

// The documentation, as pages of this console: MDX under `content/docs`, read
// at build, served at `/docs`. A console built on this one lays its own pages
// into the same tree — the hosted service's legal documents, its billing
// page — and each folder's `meta.json` ends in `...`, so they take their
// place in the book without replacing anything of the core's.
const docs = defineDocs({
  dir: "content/docs",
  docs: {
    // The processed Markdown of each page, for the corpus below.
    postprocess: { includeProcessedMarkdown: true },
  },
});

export const source = loader({
  baseUrl: "/docs",
  source: docs.toFumadocsSource(),
});

// The same pages as Markdown for a model — `/llms.txt`, the index, and
// `/llms-full.txt`, every page — which is the corpus the console agent reads.
// A page opens with its title and a `Source:` line naming its console path,
// the shape the agent's indexer keys pages on (`crates/agent/src/index/docs.rs`);
// a path rather than a URL, so an answer's link stays on this origin, whichever
// host serves it.
export const docsLlms = llms(source, {
  renderPage: async (page) =>
    `# ${page.data.title}\nSource: ${page.url}\n\n${await page.data.getText("processed")}`,
});
