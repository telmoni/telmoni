"use client";

import { Check, Copy } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { useCopyToClipboard } from "@/lib/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

export function CopyButton({
  value,
  label,
  absolute,
  labelVisible = false,
}: {
  value: string;
  label: string;
  absolute?: boolean;
  labelVisible?: boolean;
}) {
  const { copied, copy } = useCopyToClipboard();

  return (
    <Button
      type="button"
      variant="ghost"
      size={labelVisible ? "sm" : "icon"}
      className={cn("shrink-0 text-muted-foreground hover:text-foreground", labelVisible && "self-end sm:self-start")}
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
      {labelVisible && <span>{copied ? "Copied" : `Copy ${label}`}</span>}
    </Button>
  );
}
