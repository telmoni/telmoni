import type { Metadata } from "next";
import { Geist_Mono, Inter } from "next/font/google";
import { headers } from "next/headers";
import "./globals.css";
import { JsonLd } from "@/components/json-ld";
import { CORNERS_BOOT_SCRIPT } from "@/lib/corners";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { PRODUCT_DESCRIPTION, PRODUCT_NAME, SITE_URL } from "@/lib/site";

export const metadata: Metadata = {
  metadataBase: new URL(SITE_URL),
  title: {
    template: `%s · ${PRODUCT_NAME}`,
    default: PRODUCT_NAME,
  },
  description: PRODUCT_DESCRIPTION,
  robots: "index, follow",
};

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
            snaps. Carries the nonce because `proxy.ts` serves
            `script-src 'self' 'nonce-…'` and would otherwise refuse it. */}
        <script
          nonce={nonce}
          dangerouslySetInnerHTML={{ __html: CORNERS_BOOT_SCRIPT }}
        />
      </head>
      <body>
        <JsonLd />
        <ThemeProvider
          nonce={nonce}
          attribute="class"
          defaultTheme="system"
          enableSystem
          disableTransitionOnChange
        >
          {children}
          <Toaster />
        </ThemeProvider>
      </body>
    </html>
  );
}
