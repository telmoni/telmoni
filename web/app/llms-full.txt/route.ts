import { docsLlms } from "@/lib/source";

// Every page of the book as one Markdown file: the corpus the console agent
// indexes, served by the console that holds the pages, so it is always the
// deployed version's own.
export async function GET() {
  return new Response(await docsLlms.full(), {
    headers: { "Content-Type": "text/markdown; charset=utf-8" },
  });
}
