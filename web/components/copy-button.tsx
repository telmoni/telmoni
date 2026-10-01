"use client";

import { Check, Copy } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { useCopyToClipboard } from "@/lib/use-copy-to-clipboard";

export function CopyButton({
  value,
  label,
  absolute,
}: {
  value: string;
  label: string;
  absolute?: boolean;
}) {
  const { copied, copy } = useCopyToClipboard();

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-xs"
      className="text-muted-foreground hover:text-foreground"
      aria-label={`Copy ${label}`}
      onClick={async () => {
        const text = absolute ? `${window.location.origin}${value}` : value;
        if (!(await copy(text))) {
          toast.error(`Couldn't copy ${label}.`);
        }
      }}
    >
      {copied ? (
        <Check className="text-foreground" aria-hidden="true" />
      ) : (
        <Copy aria-hidden="true" />
      )}
    </Button>
  );
}
