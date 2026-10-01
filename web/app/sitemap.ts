import type { MetadataRoute } from "next";

import { SITE_URL } from "@/lib/site";

// The console's one public page. The operator's own pages — about, security,
// the legal documents — are published elsewhere and listed in that site's map.
export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: SITE_URL, changeFrequency: "monthly", priority: 1 }];
}
