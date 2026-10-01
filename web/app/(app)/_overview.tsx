import type { ReactNode } from "react";

import { PageHeader } from "@/components/page-header";
import { Card } from "@/components/ui/card";

// One content wrapper under <main>, and it is main's last child — the shape
// every console page keeps (content-containers.spec.ts). Anything a page
// shows below the summary card goes in `sections`, INSIDE the wrapper, rather
// than after it.
export function Overview({
  children,
  sections,
}: {
  children: ReactNode;
  sections?: ReactNode;
}) {
  return (
    <>
      <PageHeader title="Overview" />
      <div className="grid gap-3">
        <section className="grid gap-3">
          <Card>{children}</Card>
        </section>
        {sections}
      </div>
    </>
  );
}
