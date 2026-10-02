import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";

import { ServiceUnavailable } from "@/components/service-unavailable";

import { PageHeader } from "@/components/page-header";
import { AccessDenied } from "@/components/access-denied";
import { Card } from "@/components/ui/card";
import { ownerContact } from "@/lib/identity";
import {
  activeOrganization,
  fetchProjectBySlug,
  fetchTokens,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";
import { Role } from "@/lib/types/enums";

import { KeysActions } from "./_manage";
import { TokenRowActions } from "./_row-actions";
export const metadata = { title: "API keys" };

export default async function ApiKeysPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;
  const gate = await getServerContext();
  if (!gate) return <ServiceUnavailable />;

  const project = await fetchProjectBySlug(slug);
  if (!project) return <ServiceUnavailable />;

  const listing = await fetchTokens(project.id);
  if (listing.kind === "unavailable") return <ServiceUnavailable />;

  const canManage = project.role === Role.Owner || project.role === Role.Admin;

  if (listing.kind === "forbidden") {
    return (
      <AccessDenied
        what="this project's API keys"
        owner={ownerContact(activeOrganization(gate))}
        backHref={projectPath(organization, slug)}
      />
    );
  }
  const tokens = listing.tokens;

  return (
    <>
      <PageHeader
        title="API keys"
        action={
          canManage ? <KeysActions projectId={project.id} canManage={canManage} /> : undefined
        }
      />
      <div className="grid gap-6">
        <section className="grid gap-3">
          {/* No heading: this is the page's only section, so one would repeat
              the `PageHeader` above it word for word. Every other console page
              that carries an `h2` here uses it to name a section the title
              does NOT already cover — "Invited", "Organization notices". */}
          <p className="text-sm text-muted-foreground">
            Tokens for the CLI and API. Plaintext is shown only when minted.
          </p>
          {tokens.length === 0 ? (
            <Card className="overflow-hidden p-0 gap-0">
              <div className="grid gap-1 p-4">
                <p className="text-sm font-medium">No API keys yet.</p>
                <p className="text-sm text-muted-foreground">
                  {canManage
                    ? "Mint one to call the API."
                    : "API keys will appear here once minted."}
                </p>
              </div>
            </Card>
          ) : (
            <DataTable>
              <THead>
                <Th>Name</Th>
                <Th>Expires</Th>
                <Th>Last used</Th>
                <Th>Created</Th>
                <Th />
              </THead>
              <tbody>
                {tokens.map((t) => (
                  <tr key={t.id}>
                    <Td className="font-medium whitespace-nowrap">{t.name}</Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      <ExpiryCell expiresAt={t.expires_at} />
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      {t.last_used_at ? (
                        <LocalTime iso={t.last_used_at} mode="date" />
                      ) : (
                        "—"
                      )}
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      <LocalTime iso={t.created_at} mode="date" />
                    </Td>
                    <Td className="text-right whitespace-nowrap">
                      {canManage && (
                        <TokenRowActions
                          projectId={project.id}
                          tokenId={t.id}
                          name={t.name}
                          canManage={canManage}
                        />
                      )}
                    </Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          )}
        </section>
      </div>
    </>
  );
}

function ExpiryCell({ expiresAt }: { expiresAt: string | null }) {
  if (!expiresAt) return <span className="italic">Never</span>;
  const d = new Date(expiresAt);
  if (d < new Date()) return <span className="text-destructive">Expired</span>;
  return <LocalTime iso={expiresAt} mode="date" />;
}
