import type { MetadataRoute } from "next";

import { env } from "@/lib/env";
import { source } from "@/lib/source";

// Rendered per request, as robots.ts is: the origin is `AUTH_URL`, read at
// runtime.
export const dynamic = "force-dynamic";

// The console's public pages: the splash, and every page of the book. The
// operator's own pages — about, the legal documents — are a console built on
// this one's to list.
export default function sitemap(): MetadataRoute.Sitemap {
  return [
    { url: env.AUTH_URL, changeFrequency: "monthly", priority: 1 },
    ...source.getPages().map((page) => ({
      url: `${env.AUTH_URL}${page.url}`,
      changeFrequency: "monthly" as const,
      priority: 0.7,
    })),
  ];
}
