import { EXTRA_FOOTER_LINKS, type ExtraFooterLink } from "@/lib/extension/footer";
import {
  DISCUSSIONS_URL,
  DOCS_URL,
  REPO_URL,
  SPONSORS_URL,
  STATUS_URL,
  SUPPORT_URL,
  TWITTER_URL,
} from "@/lib/site";

export type FooterLink = { label: string; href: string; newTab?: boolean };
export type FooterColumn = { title: string; links: FooterLink[] };

type FooterDeployment = { legalUrl?: string; supportEmail?: string };

// The column an extra link names to join the legal line at the foot rather
// than a column.
const LEGAL_COLUMN = "Legal";

const docs = (label: string, path: string): FooterLink => ({
  label,
  href: `${DOCS_URL}${path}`,
});

// The footer as a product site draws one: four columns — what the product is,
// what a developer reaches for, where to turn, and who runs it — and the legal
// line at the foot (`footerLegal`). A column with nothing in it is not drawn,
// and a link exists only where its page does: the product's own links — its
// docs, its source — on every deployment, and a way to write in only where
// the deployment configured an address, so a self-hosted console offers no
// way in that it does not have. A console built on this one adds its own
// pages through `lib/extension/footer.ts`.
export function footerColumns(
  deployment: FooterDeployment,
  extras: readonly ExtraFooterLink[] = EXTRA_FOOTER_LINKS,
): FooterColumn[] {
  const { supportEmail } = deployment;
  const columns: FooterColumn[] = [
    {
      title: "Product",
      links: [
        { label: "Overview", href: "/" },
        docs("Self-hosting", "/self-host/overview"),
        ...(REPO_URL ? [{ label: "Open source", href: REPO_URL }] : []),
      ],
    },
    {
      title: "Developers",
      links: [
        { label: "Documentation", href: DOCS_URL },
        docs("CLI", "/api/cli"),
        docs("SDKs", "/api/sdks"),
        docs("API reference", "/api/reference"),
        docs("Integrations", "/integrations/notifications"),
        docs("Errors", "/errors"),
        ...(STATUS_URL ? [{ label: "Status", href: STATUS_URL, newTab: true }] : []),
      ],
    },
    {
      title: "Resources",
      links: [
        ...(SUPPORT_URL ? [{ label: "Support", href: SUPPORT_URL }] : []),
        docs("Security", "/legal/security"),
        docs("Contributing", "/contributing/introduction"),
        ...(DISCUSSIONS_URL ? [{ label: "Discussions", href: DISCUSSIONS_URL }] : []),
        ...(SPONSORS_URL ? [{ label: "Sponsor", href: SPONSORS_URL }] : []),
      ],
    },
    {
      title: "Company",
      links: [
        ...(supportEmail ? [{ label: "Contact", href: `mailto:${supportEmail}` }] : []),
        ...(TWITTER_URL ? [{ label: "Twitter", href: TWITTER_URL }] : []),
      ],
    },
  ];
  return withExtras(
    columns,
    extras.filter((extra) => extra.column !== LEGAL_COLUMN),
  ).filter((column) => column.links.length > 0);
}

// The legal line at the foot: the documents the deployment configured a base
// for, and nothing where it did not, so a self-hosted console offers no
// document it does not have.
export function footerLegal(
  deployment: FooterDeployment,
  extras: readonly ExtraFooterLink[] = EXTRA_FOOTER_LINKS,
): FooterLink[] {
  const { legalUrl } = deployment;
  const legal: FooterColumn = {
    title: LEGAL_COLUMN,
    links: legalUrl
      ? [
          { label: "Terms", href: `${legalUrl}/terms-of-service`, newTab: true },
          { label: "Privacy", href: `${legalUrl}/privacy-policy`, newTab: true },
        ]
      : [],
  };
  const [line] = withExtras(
    [legal],
    extras.filter((extra) => extra.column === LEGAL_COLUMN),
  );
  return line?.links ?? [];
}

// Each extra joins the column it names, before the link it names or last; a
// column the core does not draw is added after the core's.
function withExtras(
  columns: readonly FooterColumn[],
  extras: readonly ExtraFooterLink[],
): FooterColumn[] {
  const merged = columns.map((column) => ({ ...column, links: [...column.links] }));
  for (const extra of extras) {
    let column = merged.find((candidate) => candidate.title === extra.column);
    if (column === undefined) {
      column = { title: extra.column, links: [] };
      merged.push(column);
    }
    const link: FooterLink = {
      label: extra.label,
      href: extra.href,
      ...(extra.newTab ? { newTab: true } : {}),
    };
    const at = extra.before ? column.links.findIndex((l) => l.label === extra.before) : -1;
    if (at === -1) {
      column.links.push(link);
    } else {
      column.links.splice(at, 0, link);
    }
  }
  return merged;
}
