import { branding } from "@/lib/server/branding";
import { PRODUCT_DESCRIPTION, PRODUCT_NAME, REPO_URL, SITE_URL } from "@/lib/site";

export function JsonLd() {
  const graph = {
    "@context": "https://schema.org",
    "@graph": [
      {
        "@type": "Organization",
        "@id": `${SITE_URL}/#organization`,
        name: branding.COMPANY_NAME,
        url: SITE_URL,
        ...(REPO_URL ? { sameAs: [REPO_URL] } : {}),
      },
      {
        "@type": "SoftwareApplication",
        "@id": `${SITE_URL}/#application`,
        name: PRODUCT_NAME,
        applicationCategory: "DeveloperApplication",
        operatingSystem: "Web",
        url: SITE_URL,
        publisher: { "@id": `${SITE_URL}/#organization` },
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
