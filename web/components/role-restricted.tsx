import { Card } from "@/components/ui/card";

/**
 * The section-level refusal: one card where the listing would be, on a page
 * the caller can otherwise stand on. The rail draws every row whatever the
 * role, so this is what a member meets behind the audit log, and it
 * has to say what the role lacks and whom to ask, because the row that brought
 * them here said neither. The owner is named when `/me` knew them
 * (`ownerContact`): admins can change a role too, but the owner is the one
 * person every organization is sure to have.
 *
 * The page-level version is `AccessDenied`.
 */
export function RoleRestricted({
  what,
  scope = "project",
  owner = null,
}: {
  what: string;
  scope?: "project" | "organization";
  /// The owner's name, else their address; `null` when unknown.
  owner?: string | null;
}) {
  return (
    <Card className="grid gap-1">
      <p className="text-sm font-medium">
        Your role on this {scope} doesn&rsquo;t include {what}.
      </p>
      <p className="text-sm text-muted-foreground">
        {owner ? (
          <>
            Contact {owner}, the {scope}&rsquo;s owner, to request access.
          </>
        ) : (
          <>Contact the {scope}&rsquo;s owner to request access.</>
        )}
      </p>
    </Card>
  );
}
