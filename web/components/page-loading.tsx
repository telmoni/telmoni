import { Skeleton } from "@/components/ui/skeleton";

// A console page while it is on its way: the row a page's header stands in,
// as tall as `PageHeader`'s so nothing jumps when the page lands, and a block
// where its content will be. The console's `loading.tsx` files draw it, which
// Next shows the moment a link is followed and swaps for the page once it
// arrives — the shared layouts, the rail and the header, stay as they are.
export function PageLoading() {
  return (
    <div role="status" className="flex w-full min-w-0 flex-col gap-3">
      <span className="sr-only">Loading the page</span>
      <div className="flex min-h-9 items-center">
        <Skeleton className="h-5 w-40" />
      </div>
      <Skeleton className="h-48 w-full" />
    </div>
  );
}
