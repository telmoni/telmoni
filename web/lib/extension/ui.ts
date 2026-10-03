// The console's UI surface for a console built on this one. Anything that
// lays its own pages over this tree imports from here or from `./server`,
// never from a module path, so the modules behind these names can move
// without breaking it. Client-safe only: a client component imports this
// file, so nothing here may reach `server-only` code.
export { Button } from "@/components/ui/button";
export { Card } from "@/components/ui/card";
export {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
export {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
export { AccessDenied } from "@/components/access-denied";
export { PageAction } from "@/components/page-action";
export { PageHeader } from "@/components/page-header";
export { RoleRestricted } from "@/components/role-restricted";
export { ServiceUnavailable } from "@/components/service-unavailable";
export { DOCS_URL, PRODUCT_NAME, SPONSORS_URL } from "@/lib/site";
export { cn } from "@/lib/utils";
export { organizationLabel } from "@/lib/identity";
export { organizationPath, projectPath } from "@/lib/slug";
