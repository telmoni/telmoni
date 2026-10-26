import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";

import { PaperShell } from "@/components/paper-shell";
import { getServerSession } from "@/lib/server/session";
import { source } from "@/lib/source";

// Framed as every public page is, so the header and footer are the splash's;
// Fumadocs' navbar stays off, since the shell carries one.
//
// Fumadocs' context — the sidebar's, and the search dialog with its ⌘K — is
// the book's alone: in the root layout it fetched the dialog's code on every
// page, and ⌘K opened the book's search everywhere. The theme is the root
// layout's, so its own stays off.
// The dialog asks the book's index at its versioned route, not Fumadocs' default.
const search = { options: { api: "/api/v1/search" } };

export default async function DocsRootLayout({ children }: { children: React.ReactNode }) {
  const signedIn = (await getServerSession()) !== null;
  return (
    <RootProvider theme={{ enabled: false }} search={search}>
      <PaperShell signedIn={signedIn} framed>
        <DocsLayout tree={source.getPageTree()} nav={{ enabled: false }}>
          {children}
        </DocsLayout>
      </PaperShell>
    </RootProvider>
  );
}
