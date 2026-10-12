import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";

import { PaperShell } from "@/components/paper-shell";
import { getServerSession } from "@/lib/server/session";
import { source } from "@/lib/source";

// The documentation inside the console's own shell: the header and footer
// every public page has, with Fumadocs' sidebar and table of contents
// between them and its navbar off, since the shell carries one.
//
// Fumadocs' context — the sidebar's, and the search dialog with its ⌘K — is
// the book's alone, here rather than in the root layout: there it fetched
// the dialog's code on every page of the console, and opened the book's
// search on pages that are not the book. The theme is the root layout's, so
// its own stays off.
export default async function DocsRootLayout({ children }: { children: React.ReactNode }) {
  const signedIn = (await getServerSession()) !== null;
  return (
    <RootProvider theme={{ enabled: false }}>
      <PaperShell signedIn={signedIn}>
        <DocsLayout tree={source.getPageTree()} nav={{ enabled: false }}>
          {children}
        </DocsLayout>
      </PaperShell>
    </RootProvider>
  );
}
