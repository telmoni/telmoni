import { JsonLd } from "@/components/json-ld";
import { PaperShell } from "@/components/paper-shell";
import { getServerSession } from "@/lib/server/session";

// No `metadata` here on purpose. `layout.tsx` templates a page title as
// `%s · Telmoni`, so a string set on the home route renders the product name
// twice; its `default` is already PRODUCT_NAME, which is what this route wants.

export default async function SplashPage() {
  const session = await getServerSession();
  const signedIn = session !== null;

  return (
    <PaperShell signedIn={signedIn} className="dark">
      {/* The structured data describes this page, the one a crawler may index
          (robots.ts), and it names the deployment's origin, which is read per
          request: in the root layout it would also reach the 404 page, which
          is built once with no origin to read. */}
      <JsonLd />
      <main className="flex flex-1 flex-col">
        {/* ⚠ **The sentence IS the `<h1>`, and that is the last of the drawn
            wordmark.** The mark that used to close this page held the only
            heading on the route, inside a screen-reader span, so removing the
            drawing left the heading behind as an invisible brand word. This is
            the version that needs neither: the copy is the page's own claim,
            which is what a heading is for. `PRODUCT_NAME` is in the document
            title already and does not want saying twice.

            Four more classes went with the mark. `min-h-[calc(100svh-…)]`
            reserved the band it was drawn in and was what pushed this page
            past the viewport; `relative`, `z-10` and `pointer-events-none`
            existed only so a full-bleed layer underneath could not swallow a
            click, and there is no layer now. `flex-1` alone fills the shell. */}
        <div className="px-3.5 py-6">
          <h1 className="max-w-xl text-sm text-muted-foreground">
            Organizations and projects, members and roles, API keys, and an
            audit log you can verify.
          </h1>
        </div>
      </main>
    </PaperShell>
  );
}
