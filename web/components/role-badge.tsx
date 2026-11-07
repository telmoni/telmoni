import { Badge } from "@/components/ui/badge";
import { roleLabel } from "@/lib/role-label";
import type { OrganizationRole, Role } from "@/lib/types/enums";

/** All roles use the same fill; the label carries their rank. */
export function RoleBadge({
  role,
  testId,
  className,
}: {
  role: Role | OrganizationRole;
  testId?: string;
  className?: string;
}) {
  return (
    <Badge
      variant="secondary"
      className={className}
      data-testid={testId}
    >
      {roleLabel(role)}
    </Badge>
  );
}
