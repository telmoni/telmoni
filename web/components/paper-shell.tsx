import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { cn } from "@/lib/utils";

// The public pages' shell: the header and footer every one of them has.
// `framed` is the bordered column every public page takes, its header and
// footer inset by the sections' measure (`lib/inset.ts`); unframed, the shell
// runs edge to edge as the console does.
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
