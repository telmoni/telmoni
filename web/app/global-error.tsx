"use client";

import { PRODUCT_NAME } from "@/lib/site";
import { geistMono } from "@/lib/fonts";
import { SettingsBoot } from "@/components/settings-boot";
import { ThemeProvider } from "@/components/theme-provider";
import { Button } from "@/components/ui/button";
import "./globals.css";

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
    <html lang="en" className={geistMono.variable} suppressHydrationWarning>
      <body>
        <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
          <title>{`Something went wrong · ${PRODUCT_NAME}`}</title>
          <main className="flex min-h-svh w-full max-w-xl flex-col items-start gap-gutter px-page py-24">
            <p className="text-xs tracking-label text-destructive uppercase">critical error</p>
            <h1 className="text-3xl font-medium tracking-tight">Something went wrong.</h1>
            <p className="text-sm text-muted-foreground">
              {error.message || "A critical error occurred. We have been notified."}
            </p>
            {error.digest && <p className="text-xs text-muted-foreground">ref: {error.digest}</p>}
            <Button type="button" onClick={() => retry()}>Try again</Button>
          </main>
          {/* The root layout's boot script never runs here: corners and accessibility come from this. */}
          <SettingsBoot />
        </ThemeProvider>
      </body>
    </html>
  );
}
