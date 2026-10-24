import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { cn } from "@/lib/utils";

// The public pages' shell: the header and footer every one of them has.
// `framed` is the splash's column — one bordered column holding the header,
// the page and the footer, ruled between, each inset from the column's edge
// by one measure (`lib/inset.ts`) — and every public page's but the book's,
// which runs the shell edge to edge as the console does.
export function PaperShell({
  children,
  signedIn,
  className,
  framed = false,
}: {
  children: React.ReactNode;
  signedIn: boolean;
  className?: string;
  framed?: boolean;
}) {
  return (
    <div
      className={cn(
        // `bg-background` is not decoration: this element DECLARES
        // `theme-paper` and so must paint it. Without it the only painted
        // surface was `SiteHeader`, and every gap fell through to `body`,
        // whose `bg-background` resolves in the root theme instead. In dark
        // that is a visibly deeper colour than the paper it sits behind.
        "theme-paper relative min-h-screen bg-background",
        className
      )}
    >
      <div
        className={cn(
          "flex min-h-screen flex-col",
          framed && "mx-auto w-full max-w-[1320px] border-x border-border",
        )}
      >
        <SiteHeader signedIn={signedIn} inset={framed} />
        <div className="flex-1 flex flex-col">{children}</div>
        <SiteFooter inset={framed} />
      </div>
    </div>
  );
}
