import { redirect } from "next/navigation";

import { organizationPath } from "@/lib/slug";

// An organization opens on its projects, the page every member of it can read.
export default async function OrganizationPage({ params }: PageProps<"/[organization]">) {
  const { organization } = await params;
  redirect(organizationPath(organization, "/projects"));
}
