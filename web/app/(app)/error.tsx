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
    <div className="flex flex-col items-start gap-4 max-w-md py-8">
      <p className="text-xs tracking-label text-brand-negative uppercase">error</p>
      <h1 className="text-xl font-light tracking-tight">This page hit a problem.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        {error.message || "An unexpected error occurred."}
      </p>
      {error.digest && (
        <p className="text-xs text-muted-foreground/50">ref: {error.digest}</p>
      )}
      <Button variant="outline" onClick={() => retry()}>Try again</Button>
    </div>
  );
}
