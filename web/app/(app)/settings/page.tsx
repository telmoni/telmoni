import { redirect } from "next/navigation";

// The section's root opens its first page.
export default function SettingsPage() {
  redirect("/settings/general");
}
