import { PageLoading } from "@/components/page-loading";

// From one of a project's pages to another, the project's layout stays and
// this stands in for the page while it loads.
export default function Loading() {
  return <PageLoading />;
}
