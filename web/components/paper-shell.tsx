import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { cn } from "@/lib/utils";

export function PaperShell({
  children,
  signedIn,
  className,
}: {
  children: React.ReactNode;
  signedIn: boolean;
  className?: string;
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
      <div className="flex min-h-screen flex-col">
        <SiteHeader signedIn={signedIn} />
        <div className="flex-1 flex flex-col">{children}</div>
        <SiteFooter />
      </div>
    </div>
  );
}
