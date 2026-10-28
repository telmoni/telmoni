import { Badge } from "@/components/ui/badge";
import { roleLabel } from "@/lib/role-label";
import type { OrganizationRole, Role } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

/**
 * A person's standing on a project or an organization, as one badge.
 *
 * ⚠ **Filled for every role, and that is the whole reason this is one
 * component.** The variant used to swing on `role === "owner"` — filled for an
 * owner, outlined for everyone else — which made the FILL a second signal for
 * rank on top of the word that already carries it, so an outlined "Admin" beside
 * a filled "Owner" read as two kinds of thing rather than two roles. The rule
 * lived in two places and they disagreed; now it lives here.
 */
export function RoleBadge({
  role,
  testId,
  className,
}: {
  role: Role | OrganizationRole;
  testId?: string;
  /// For density only — the dropdown's rows are tighter than a table cell's.
  /// Never for the variant: that is this component's to decide.
  className?: string;
}) {
  return (
    <Badge
      variant="secondary"
      className={cn("text-[10px] font-normal shrink-0", className)}
      data-testid={testId}
    >
      {roleLabel(role)}
    </Badge>
  );
}
