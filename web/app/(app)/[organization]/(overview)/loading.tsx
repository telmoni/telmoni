import { PageLoading } from "@/components/page-loading";

// From one of an organization's pages to another, the organization's layout
// stays and this stands in for the page while it loads. The overview sits in
// a route group of its own so this wraps the overview alone, and each of the
// organization's other pages has its own: one beside the organization's
// layout would wrap the project's layout too, whose redirect to a project's
// canonical path and whose not-found would then arrive inside a page already
// answered 200, rather than as a 307 and a 404.
export default function Loading() {
  return <PageLoading />;
}
