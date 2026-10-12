"use client";
import { Button } from "@/components/ui/button";

// "Try again" is `retry`, which fetches the segment afresh: `reset` would
// draw the same failed answer again.
export default function GlobalError({
  error,
  retry,
}: {
  error: Error & { digest?: string };
  retry: () => void;
}) {
  return (
    <div className="flex flex-col items-start px-8 py-24 gap-6 max-w-md">
      <p className="text-xs tracking-label text-brand-negative uppercase">error</p>
      <h1 className="text-3xl font-light tracking-tight">Something went wrong.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        {error.message || "An unexpected error occurred."}
      </p>
      {error.digest && (
        <p className="text-xs text-muted-foreground/50 font-mono">ref: {error.digest}</p>
      )}
      <Button variant="outline" onClick={() => retry()}>Try again</Button>
    </div>
  );
}
