import { DocsLayout } from "fumadocs-ui/layouts/docs";

import { PaperShell } from "@/components/paper-shell";
import { getServerSession } from "@/lib/server/session";
import { source } from "@/lib/source";

// The documentation inside the console's own shell: the header and footer
// every public page has, with Fumadocs' sidebar and table of contents
// between them and its navbar off, since the shell carries one.
export default async function DocsRootLayout({ children }: { children: React.ReactNode }) {
  const signedIn = (await getServerSession()) !== null;
  return (
    <PaperShell signedIn={signedIn}>
      <DocsLayout tree={source.getPageTree()} nav={{ enabled: false }}>
        {children}
      </DocsLayout>
    </PaperShell>
  );
}
