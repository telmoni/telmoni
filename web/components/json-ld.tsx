import { env } from "@/lib/env";
import { branding } from "@/lib/server/branding";
import { PRODUCT_DESCRIPTION, PRODUCT_NAME, REPO_URL } from "@/lib/site";

export function JsonLd() {
  // The deployment's own origin, read at request time as `branding` is, so
  // one image serves every operator.
  const origin = env.AUTH_URL;
  const graph = {
    "@context": "https://schema.org",
    "@graph": [
      {
        "@type": "Organization",
        "@id": `${origin}/#organization`,
        name: branding.COMPANY_NAME,
        url: origin,
        ...(REPO_URL ? { sameAs: [REPO_URL] } : {}),
      },
      {
        "@type": "SoftwareApplication",
        "@id": `${origin}/#application`,
        name: PRODUCT_NAME,
        applicationCategory: "DeveloperApplication",
        operatingSystem: "Web",
        url: origin,
        publisher: { "@id": `${origin}/#organization` },
        description: PRODUCT_DESCRIPTION,
      },
    ],
  };

  return (
    <script
      type="application/ld+json"
      dangerouslySetInnerHTML={{
        __html: JSON.stringify(graph).replace(/</g, "\\u003c"),
      }}
    />
  );
}
