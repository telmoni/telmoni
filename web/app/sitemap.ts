import type { MetadataRoute } from "next";

import { env } from "@/lib/env";

// Rendered per request, as robots.ts is: the origin is `AUTH_URL`, read at
// runtime.
export const dynamic = "force-dynamic";

// The console's one public page. The operator's own pages — about, security,
// the legal documents — are published elsewhere and listed in that site's map.
export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: env.AUTH_URL, changeFrequency: "monthly", priority: 1 }];
}
