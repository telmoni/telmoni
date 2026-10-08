import { cn } from "@/lib/utils";

// The one phrase a heading leans on, drawn as a highlighter stroke: the
// product site's one accent, on a page that is otherwise ink and paper. The
// stroke is drawn rather than there (`globals.css`, *The splash's motion*):
// the hero's once its claim has settled, a section's as its heading scrolls
// into view.
export function Mark({
  children,
  sweep = "view",
}: {
  children: React.ReactNode;
  sweep?: "load" | "view";
}) {
  return (
    <mark
      className={cn(
        "mark-stroke rounded-xs px-1 text-highlight-foreground",
        sweep === "load" ? "mark-sweep-load" : "mark-sweep-view",
      )}
    >
      {children}
    </mark>
  );
}
