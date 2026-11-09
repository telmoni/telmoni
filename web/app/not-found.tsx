import Link from "next/link";
import type { Metadata } from "next";

import { Button } from "@/components/ui/button";

export const metadata: Metadata = {
  title: "404 Not Found",
  robots: "noindex",
};

export default function NotFound() {
  return (
    <div className="flex max-w-md flex-col items-start gap-page px-page py-24">
      <p className="text-xs tracking-label text-muted-foreground uppercase">404</p>
      <h1 className="text-xl font-medium tracking-tight">Page not found.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        The page you&apos;re looking for doesn&apos;t exist or has been moved.
      </p>
      <div>
        <Button asChild variant="outline">
          <Link href="/">Back home</Link>
        </Button>
      </div>
    </div>
  );
}
