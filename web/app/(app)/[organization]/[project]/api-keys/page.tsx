import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";

import { ServiceUnavailable } from "@/components/service-unavailable";

import { PageHeader } from "@/components/page-header";
import { AccessDenied } from "@/components/access-denied";
import { Badge } from "@/components/ui/badge";
import { NoticeCard } from "@/components/notice-card";
import { ownerContact } from "@/lib/identity";
import { activeOrganization, fetchTokens } from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { SCOPE_LABEL } from "@/lib/api-key-scopes";
import { projectPath } from "@/lib/slug";
import { administers } from "@/lib/roles";

import { KeysActions } from "./_manage";
import { TokenRowActions } from "./_row-actions";
export const metadata = { title: "API keys" };

export default async function ApiKeysPage({
  params,
}: PageProps<"/[organization]/[project]/api-keys">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;

  const listing = await fetchTokens(project.id);
  if (listing.kind === "unavailable") return <ServiceUnavailable />;

  const canManage = administers(project.role);

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
          canManage ? <KeysActions projectId={project.id} /> : undefined
        }
      />
      <div className="grid gap-6">
        <section className="grid gap-3">
          {/* No heading: this is the page's only section, so one would repeat
              the `PageHeader` above it word for word. Every other console page
              that carries an `h2` here uses it to name a section the title
              does NOT already cover — "Invited", "Organization notices". */}
          <p className="text-sm text-muted-foreground">
            Keys for sending spans and reading the API, each named by its first characters; the
            plaintext is shown only when minted.
          </p>
          {tokens.length === 0 ? (
            <NoticeCard
              title="No API keys yet."
              body={
                canManage ? "Mint one to call the API." : "API keys will appear here once minted."
              }
            />
          ) : (
            <DataTable>
              <THead>
                <Th>Name</Th>
                <Th>Key</Th>
                <Th>Opens</Th>
                <Th>Expires</Th>
                <Th>Last used</Th>
                <Th>Created</Th>
                <Th />
              </THead>
              <tbody>
                {tokens.map((t) => (
                  <tr key={t.id}>
                    <Td className="font-medium whitespace-nowrap">{t.name}</Td>
                    <Td className="whitespace-nowrap">
                      <Badge variant="secondary">{t.tokenHint}…</Badge>
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      {t.scopes.map((s) => SCOPE_LABEL[s]).join(", ")}
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      <ExpiryCell expiresAt={t.expiresAt} />
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      {t.lastUsedAt ? (
                        <LocalTime iso={t.lastUsedAt} mode="date" />
                      ) : (
                        "—"
                      )}
                    </Td>
                    <Td className="text-xs text-muted-foreground whitespace-nowrap">
                      <LocalTime iso={t.createdAt} mode="date" />
                    </Td>
                    <Td className="text-right whitespace-nowrap">
                      {canManage && (
                        <TokenRowActions projectId={project.id} tokenId={t.id} name={t.name} />
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
