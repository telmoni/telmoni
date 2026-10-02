import {
  DISCUSSIONS_URL,
  DOCS_URL,
  REPO_URL,
  STATUS_URL,
  TWITTER_URL,
} from "@/lib/site";

type FooterLink = { label: string; href: string; newTab?: boolean };
type FooterColumn = { title: string; links: FooterLink[] };

// A column with nothing in it is not drawn: the legal and contact columns
// exist only where the deployment configured a legal base or an address, so
// a self-hosted console offers no document, and no way to write in, that it
// does not have. The product's own links — its docs, its source — are
// resources, on every deployment.
export function footerColumns(deployment: {
  legalUrl?: string;
  supportEmail?: string;
}): FooterColumn[] {
  const { legalUrl, supportEmail } = deployment;
  const columns: FooterColumn[] = [
    {
      title: "Resources",
      links: [
        { label: "Docs", href: DOCS_URL, newTab: true },
        ...(REPO_URL ? [{ label: "GitHub", href: REPO_URL }] : []),
        ...(STATUS_URL ? [{ label: "Status", href: STATUS_URL, newTab: true }] : []),
      ],
    },
    {
      title: "Legal",
      links: legalUrl
        ? [
            { label: "Privacy Policy", href: `${legalUrl}/privacy-policy`, newTab: true },
            { label: "Terms of Service", href: `${legalUrl}/terms-of-service`, newTab: true },
          ]
        : [],
    },
    {
      title: "Contact",
      links: [
        ...(supportEmail ? [{ label: "Email", href: `mailto:${supportEmail}` }] : []),
        ...(TWITTER_URL ? [{ label: "Twitter", href: TWITTER_URL }] : []),
        ...(DISCUSSIONS_URL ? [{ label: "Discussions", href: DISCUSSIONS_URL }] : []),
      ],
    },
  ];
  return columns.filter((column) => column.links.length > 0);
}
