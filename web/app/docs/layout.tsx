import { RootProvider } from "fumadocs-ui/provider/next";

import { navLinks } from "@/components/site-nav";
import { footerColumns } from "@/lib/footer";
import { branding } from "@/lib/server/branding";
import { getServerSession } from "@/lib/server/session";
import { source } from "@/lib/source";

import { DocsShell } from "./_docs-shell";

// The book's layout: a single left rail with
// the brand mark, DOCS switcher, and page tree; the article in the
// center column; and the Search trigger + Contents table of contents in the
// right column.
//
// Fumadocs' context — the search dialog with its ⌘K — is the book's alone:
// in the root layout it fetched the dialog's code on every page, and ⌘K
// opened the book's search everywhere. The theme is the root layout's, so
// its own stays off.
const search = { options: { api: "/api/v1/search" } };

const ESSENTIAL_LABELS = new Set(["Status"]);

export default async function DocsRootLayout({ children }: { children: React.ReactNode }) {
  const signedIn = (await getServerSession()) !== null;
  const primary = navLinks();
  const primaryHrefs = new Set(primary.map((l) => l.href));
  const deployment = { legalUrl: branding.LEGAL_URL, supportEmail: branding.SUPPORT_EMAIL };
  const metaLinks = footerColumns(deployment)
    .flatMap((col) => col.links)
    .filter((l) => ESSENTIAL_LABELS.has(l.label) && !primaryHrefs.has(l.href));

  return (
    <RootProvider theme={{ enabled: false }} search={search}>
      <DocsShell
        tree={source.getPageTree()}
        signedIn={signedIn}
        primaryLinks={primary}
        metaLinks={metaLinks}
        companyName={branding.COMPANY_NAME}
      >
        {children}
      </DocsShell>
    </RootProvider>
  );
}
