import type { MetadataRoute } from "next";
import { connection } from "next/server";

import { env } from "@/lib/env";

// Rendered per request (`connection()`): the console's origin, `AUTH_URL`,
// and whether it is meant to be found, `SEARCH_INDEXING`, are read at runtime
// so one image serves every deployment.
export default async function robots(): Promise<MetadataRoute.Robots> {
  await connection();
  if (!env.SEARCH_INDEXING) {
    return { rules: [{ userAgent: "*", disallow: ["/"] }] };
  }
  return {
    rules: [
      {
        userAgent: "*",
        allow: ["/"],
        // A rule is a prefix of the paths it keeps crawlers off: an
        // invitation's page is `/invite/{token}`.
        disallow: ["/api/", "/auth/", "/console", "/invite/"],
      },
    ],
    sitemap: `${env.AUTH_URL}/sitemap.xml`,
  };
}
