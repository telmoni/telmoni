import type { ReactNode } from "react";

import { CopyButton } from "@/components/copy-button";
import { Row } from "@/components/rows";
import { cn } from "@/lib/utils";

export function SettingsRow({
  label,
  children,
  mono,
  copy,
  copyAbsolute,
}: {
  label: string;
  children: ReactNode;
  mono?: boolean;
  copy?: string;
  copyAbsolute?: boolean;
}) {
  return (
    <Row label={label}>
      <span className={cn("min-w-0 break-all", mono && "font-mono text-xs")}>{children}</span>
      {copy !== undefined && <CopyButton value={copy} label={label} absolute={copyAbsolute} />}
    </Row>
  );
}
