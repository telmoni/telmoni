import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Rows } from "@/components/rows";
import { SettingsRow } from "@/components/settings-row";
import { Card } from "@/components/ui/card";
import { env } from "@/lib/env";
import { organizationLabel, ownerContact } from "@/lib/identity";
import { administersOrganization } from "@/lib/organization-role";
import { activeOrganization, getServerContext, identityContext } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";
import { organizationPath } from "@/lib/slug";

import { DeleteOrganizationForm } from "./_delete-organization";
import { OrganizationNameForm } from "./_organization-name";
import { OrganizationUrlForm } from "./_organization-url";

export const metadata = { title: "Settings" };

export default async function OrganizationSettingsPage() {
  const session = await getServerSession();
  if (!session) notFound();

  // One source for both what the page shows and what its actions send:
  // `identityContext` is `/me`'s active organization — the one the path names
  // — and every form is handed its id and acts on that one or on nothing: not
  // on whichever one their request resolves once the page has gone stale.
  // Reading one organization and writing another is how this page once showed
  // one name in a box that renamed a different organization.
  const [gate, ident] = await Promise.all([getServerContext(), identityContext()]);
  // No `/me` is an outage, not a refusal: said as one, as every sibling page
  // says it, rather than telling an owner they lack access.
  if (!gate) return <ServiceUnavailable />;
  // The path names the organization: not found unless auth answered with it.
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);

  // An ADMIN edits the name and the URL alongside the owner
  // (`can_manage_org_settings`). A MEMBER holds no organization-wide powers,
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
          description="Your organization's visible name within Telmoni — your company or group, not a person's name. Everyone in it sees it: the rail, the resource selector and every invitation it sends. Renaming moves nothing: its address stays as it is."
        >
          <OrganizationNameForm
            organizationId={organization.organizationId}
            initialName={organization.name}
            canEdit={canManage}
          />
        </Section>

        <Section
          title="URL"
          description="Your organization's address on Telmoni, where its pages and every project in it live. Lowercase letters and digits, in words joined by hyphens. Changing it moves every link to those pages: the old address stops working."
        >
          <OrganizationUrlForm
            organizationId={organization.organizationId}
            slug={organization.slug}
            host={new URL(env.AUTH_URL).host}
            canEdit={canManage}
          />
        </Section>

        <Section
          title="Organization ID"
          description="Your organization's identifier within Telmoni, for the API and the CLI. It never changes."
        >
          <Rows>
            <SettingsRow label="Organization ID" mono copy={organization.organizationId}>
              {organization.organizationId}
            </SettingsRow>
          </Rows>
        </Section>

        {isOwner && (
          <Section
            title="Danger zone"
            danger
            description="Closes this organization for everyone in it the moment you confirm: access ends, its API keys stop working, and its projects, keys and everything else it held are erased for good within the hour. There is no undo. Your account stays."
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
