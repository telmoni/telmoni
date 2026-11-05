import type { ReactNode } from "react";

import { PageHeader } from "@/components/page-header";

// One content wrapper under <main>, and it is main's last child — the shape
// every console page keeps (content-containers.spec.ts). A project's overview
// is titled by the project's name, and anything it shows under the title goes
// in `sections`, INSIDE the wrapper, rather than after it.
export function Overview({
  title,
  sections,
  action,
}: {
  title: string;
  sections?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <>
      <PageHeader title={title} action={action} />
      <div className="grid gap-page">{sections}</div>
    </>
  );
}
