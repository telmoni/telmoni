import Link from "next/link";
import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "404 Not Found",
  robots: "noindex",
};

export default function NotFound() {
  return (
    <div className="flex flex-col px-8 py-24 gap-6 max-w-md">
      <p className="text-xs tracking-label text-muted-foreground uppercase">404</p>
      <h1 className="text-3xl font-light tracking-tight">Page not found.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        The page you&apos;re looking for doesn&apos;t exist or has been moved.
      </p>
      <div>
        <Link
          href="/"
          className="text-sm rounded-md border border-border px-4 py-2.5 hover:bg-secondary transition-colors"
        >
          Back home
        </Link>
      </div>
    </div>
  );
}
