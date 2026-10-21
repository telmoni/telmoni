import type { Metadata } from "next";
import { Geist_Mono, Inter } from "next/font/google";
import { headers } from "next/headers";
import "./globals.css";
import { ACCESSIBILITY_BOOT_SCRIPT } from "@/lib/accessibility";
import { CORNERS_BOOT_SCRIPT } from "@/lib/corners";
import { FragmentEntries } from "@/components/fragment-entries";
import { SettingsBoot } from "@/components/settings-boot";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { env } from "@/lib/env";
import { PRODUCT_DESCRIPTION, PRODUCT_NAME } from "@/lib/site";

// A function, not a constant: `metadataBase` is the deployment's origin, which
// is `AUTH_URL` at request time and nothing a build could carry. Without it,
// Next resolves a relative social image, which a console built on this one may
// add as `opengraph-image`, against localhost.
export function generateMetadata(): Metadata {
  return {
    metadataBase: new URL(env.AUTH_URL),
    title: {
      template: `%s · ${PRODUCT_NAME}`,
      default: PRODUCT_NAME,
    },
    description: PRODUCT_DESCRIPTION,
    robots: "index, follow",
  };
}

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

const inter = Inter({
  variable: "--font-inter",
  subsets: ["latin"],
});

export default async function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  return (
    <html
      lang="en"
      suppressHydrationWarning
      className={`${geistMono.variable} ${inter.variable}`}
    >
      <head>
        {/* Before first paint, so a sharp console never renders rounded and
            snaps, and the accessibility settings hold from the first frame;
            carries the nonce `proxy.ts`'s CSP asks for. A 404 answered from a
            layout's `notFound()` is rendered from scratch in the browser,
            where this never runs: `SettingsBoot` does its work there. */}
        <script
          nonce={nonce}
          dangerouslySetInnerHTML={{ __html: CORNERS_BOOT_SCRIPT + ACCESSIBILITY_BOOT_SCRIPT }}
        />
      </head>
      <body>
        <ThemeProvider
          nonce={nonce}
          attribute="class"
          defaultTheme="system"
          enableSystem
          disableTransitionOnChange
        >
          {children}
          <Toaster />
          <FragmentEntries />
          <SettingsBoot />
        </ThemeProvider>
      </body>
    </html>
  );
}
