import { PageHeader } from "@/components/page-header";

import { GeneralSettings } from "./_settings";

export const metadata = { title: "General" };

// The person's look of the console, applied the moment it changes. Saved in
// this browser, as the accessibility settings are, so each device keeps its own.
export default function GeneralPage() {
  return (
    <>
      <PageHeader title="General" />
      <GeneralSettings />
    </>
  );
}
