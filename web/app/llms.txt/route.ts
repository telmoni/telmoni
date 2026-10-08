import { docsLlms } from "@/lib/source";

// The book's index as Markdown, for a model: each page's title, address and
// description.
export async function GET() {
  return new Response(await docsLlms.index(), {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
}
