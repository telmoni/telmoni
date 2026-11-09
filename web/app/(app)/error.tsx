"use client";

import { Button } from "@/components/ui/button";

// "Try again" is `retry`, which fetches the page afresh: `reset` would draw
// the same failed answer again.
export default function ConsoleError({
  error,
  retry,
}: {
  error: Error & { digest?: string };
  retry: () => void;
}) {
  return (
    <div className="flex max-w-md flex-col items-start gap-page py-page">
      <p className="text-xs tracking-label text-brand-negative uppercase">error</p>
      <h1 className="text-xl font-medium tracking-tight">This page hit a problem.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        {error.message || "An unexpected error occurred."}
      </p>
      {error.digest && (
        <p className="text-xs text-muted-foreground">ref: {error.digest}</p>
      )}
      <Button variant="outline" onClick={() => retry()}>Try again</Button>
    </div>
  );
}
