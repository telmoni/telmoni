import { PageHeader } from "@/components/page-header";
import { branding } from "@/lib/server/branding";

import { AccessibilitySettings } from "./_settings";

export const metadata = { title: "Accessibility" };

// The person's accessibility settings, each applied the moment it changes so
// its effect is seen here. Saved in this browser, as the theme and corners
// are, so each device keeps its own (`lib/accessibility.ts`), and applied to
// the console, the docs and the site alike.
export default function AccessibilityPage() {
  return (
    <>
      <PageHeader title="Accessibility" />
      <AccessibilitySettings supportEmail={branding.SUPPORT_EMAIL ?? null} />
    </>
  );
}
