import { PageLoading } from "@/components/page-loading";

// Stands in for the page while it loads: this page's own, not one beside the
// organization's layout (see `../(overview)/loading.tsx`).
export default function Loading() {
  return <PageLoading />;
}
