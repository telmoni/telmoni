import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { organizationLabel, ownerContact } from "@/lib/identity";
import { administersOrganization } from "@/lib/organization-role";
import { activeOrganization, getServerContext, identityContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";

import { DeleteOrganizationForm } from "./_delete-organization";
import { RenameOrganizationForm } from "./_rename-organization";

export const metadata = { title: "Settings" };

export default async function OrganizationSettingsPage() {
  const session = await getServerSession();
  if (!session) notFound();

  // One source for both what the page shows and what its actions send:
  // `identityContext` is `/me`'s active organization — the one the path names
  // — and the rename and the deletion are handed its id and act on that one or
  // on nothing: not on whichever one their request resolves once the page has
  // gone stale. Reading one organization and writing another is how this page
  // once showed one name in a box that renamed a different organization.
  const [gate, ident] = await Promise.all([getServerContext(), identityContext()]);
  // No `/me` is an outage, not a refusal: said as one, as every sibling page
  // says it, rather than telling an owner they lack access.
  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);

  // An ADMIN keeps the read-only view: they hold organization-wide powers,
  // just not these (`can_manage_org_settings` is Owner). A MEMBER holds none,
  // and organization settings are not a thing they have a reading interest in
  // either.
  if (!organization || !ident || !administersOrganization(ident.role)) {
    return (
      <AccessDenied
        what="this organization's settings"
        owner={ownerContact(organization)}
        backHref={organization ? organizationPath(organization.slug) : "/console"}
      />
    );
  }

  const isOwner = ident.role === "owner";
  const canManage = isOwner || ident.role === "admin";
  const label = organizationLabel(organization);

  return (
    <>
      <PageHeader title="Settings" />
      <div className="grid gap-6">
        <Section
          title="Name"
          description="What this organization is called across the console — the rail, the resource selector, and every invitation it sends. Everyone in it sees it; it is not a person's name. It needs one before it can be handed over. Its address follows the name: a rename moves every link to its pages, and a name with no Latin letters or digits keeps the address it has."
        >
          <RenameOrganizationForm
            organizationId={organization.organizationId}
            initialName={organization.name ?? ""}
            // ⚠️ **The owner's address, which is what the console actually
            // shows for an organization nobody has named** — and the reason
            // auth refuses to hand one over: the address would change hands
            // with it.
            fallbackLabel={organization.ownerEmail ?? ""}
            canEdit={canManage}
          />
        </Section>

        {isOwner && (
          <Section
            title="Danger zone"
            danger
            description="Closes this organization for everyone in it the moment you confirm: access ends and its API keys stop working. You have 14 days to restore it from your account's Privacy page; after that its projects, keys and everything else it held are erased for good. Your account stays."
          >
            <Card className="grid gap-3 bg-destructive/5 text-sm">
              <DeleteOrganizationForm
                organizationId={organization.organizationId}
                organization={label}
                email={session.email}
              />
            </Card>
          </Section>
        )}
      </div>
    </>
  );
}
