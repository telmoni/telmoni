import type { ReactNode } from "react";

import { CopyButton } from "@/components/copy-button";
import { Row } from "@/components/rows";

export function SettingsRow({
  label,
  children,
  copy,
  copyAbsolute,
}: {
  label: string;
  children: ReactNode;
  copy?: string;
  copyAbsolute?: boolean;
}) {
  return (
    <Row label={label}>
      <span className="min-w-0 break-all">{children}</span>
      {copy !== undefined && <CopyButton value={copy} label={label} absolute={copyAbsolute} />}
    </Row>
  );
}
