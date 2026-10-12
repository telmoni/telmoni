import { docsLlms } from "@/lib/source";

// Built once, at build time, and served as built: the book is the build's
// own, and its addresses are paths, so no request could change a word of it.
export const dynamic = "force-static";

// The book's index as Markdown, for a model: each page's title, address and
// description.
export async function GET() {
  return new Response(await docsLlms.index(), {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
}
