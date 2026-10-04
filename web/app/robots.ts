import type { MetadataRoute } from "next";

import { env } from "@/lib/env";

// Rendered per request: the console's origin is `AUTH_URL`, read at runtime so
// one image serves every deployment, and a file built once would carry the
// build machine's.
export const dynamic = "force-dynamic";

export default function robots(): MetadataRoute.Robots {
  return {
    rules: [
      {
        userAgent: "*",
        allow: ["/"],
        disallow: ["/api/", "/auth/", "/console", "/invites"],
      },
    ],
    sitemap: `${env.AUTH_URL}/sitemap.xml`,
  };
}
