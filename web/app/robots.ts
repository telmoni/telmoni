import type { MetadataRoute } from "next";
import { connection } from "next/server";

import { env } from "@/lib/env";

// Rendered per request (`connection()`): the console's origin is `AUTH_URL`,
// read at runtime so one image serves every deployment, and a file built once
// would carry the build machine's.
export default async function robots(): Promise<MetadataRoute.Robots> {
  await connection();
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
