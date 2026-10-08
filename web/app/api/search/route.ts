import { createFromSource } from "fumadocs-core/search/server";

import { source } from "@/lib/source";

// Full-text search over the book, for the dialog Fumadocs opens from its
// sidebar: an index built from the pages at first use, answered here.
export const { GET } = createFromSource(source, { language: "english" });
