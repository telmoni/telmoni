"use client";

import { PRODUCT_NAME } from "@/lib/site";

// "Try again" is `retry`, which fetches the page afresh: `reset` would draw
// the same failed answer again. The tab's title is React's `<title>`: this
// stands in for the root layout, so the Metadata API's title, the root
// layout's template among it, cannot reach it, and a client file may export
// no metadata of its own.
export default function GlobalError({
  error,
  retry,
}: {
  error: Error & { digest?: string };
  retry: () => void;
}) {
  return (
    <html lang="en">
      <body style={{ fontFamily: "monospace", padding: "6rem 4vw", maxWidth: "36rem" }}>
        <title>{`Something went wrong · ${PRODUCT_NAME}`}</title>
        <p style={{ fontSize: "10px", letterSpacing: "0.4em", textTransform: "uppercase", color: "#ef4444" }}>
          critical error
        </p>
        <h1 style={{ fontSize: "1.875rem", fontWeight: 300, letterSpacing: "-0.025em", margin: "1rem 0" }}>
          Something went wrong.
        </h1>
        <p style={{ fontSize: "0.875rem", color: "#888", lineHeight: 1.6 }}>
          {error.message || "A critical error occurred. We have been notified."}
        </p>
        {error.digest && (
          <p style={{ fontSize: "0.75rem", color: "#555", fontFamily: "monospace", marginTop: "0.5rem" }}>
            ref: {error.digest}
          </p>
        )}
        <button
          type="button"
          onClick={() => retry()}
          style={{
            marginTop: "1.5rem",
            border: "1px solid #333",
            padding: "0.5rem 1rem",
            fontSize: "0.875rem",
            background: "transparent",
            cursor: "pointer",
            color: "inherit",
          }}
        >
          Try again
        </button>
      </body>
    </html>
  );
}
