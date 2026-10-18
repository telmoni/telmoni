import { Role } from "@/lib/types/enums";

/** Whether a project role may change the project — its keys, members,
 *  connectors, settings and saved views — and read its audit log: Admin and
 *  up, as `crates/shared/src/rbac.rs` has it. */
export function administersProject(role: Role | null): boolean {
  return role === Role.Owner || role === Role.Admin;
}
