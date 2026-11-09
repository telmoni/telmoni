"use client";

import { ChevronDown, ChevronUp } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useCopyToClipboard } from "@/lib/use-copy-to-clipboard";

const TERMINAL_COMMANDS = [
  {
    label: "curl",
    command: "curl -fsSL https://telmoni.com/install.sh | sh",
  },
  {
    label: "cargo",
    command: "cargo install --locked telmoni-cli",
  },
] as const;

export function TerminalInstallMenu() {
  const [open, setOpen] = useState(false);
  const [copiedLabel, setCopiedLabel] = useState<string | null>(null);
  const { copied, copy } = useCopyToClipboard(1500);

  const handleCopy = async (label: string, command: string) => {
    if (await copy(command)) {
      setCopiedLabel(label);
    } else {
      toast.error("Couldn't copy the install command.");
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button type="button" size="sm" variant="outline">
          <span>Install</span>
          {open ? (
            <ChevronUp aria-hidden="true" />
          ) : (
            <ChevronDown aria-hidden="true" />
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="start"
        aria-label="Terminal install commands"
        className="flex w-max max-w-[calc(100vw-2*var(--spacing-gutter))] flex-col gap-gutter p-gutter text-sm"
      >
        {TERMINAL_COMMANDS.map((item) => (
          <div key={item.label} className="flex items-center justify-between gap-gutter">
            <div className="flex min-w-0 items-center gap-gutter">
              <span className="w-12 shrink-0 text-muted-foreground">{item.label}</span>
              <code className="truncate text-foreground select-all" title={item.command}>
                {item.command}
              </code>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-label={`Copy ${item.label} install command`}
              onClick={() => void handleCopy(item.label, item.command)}
              className="shrink-0 text-muted-foreground hover:text-foreground"
            >
              {copied && copiedLabel === item.label ? "Copied" : "Copy"}
            </Button>
          </div>
        ))}
      </PopoverContent>
    </Popover>
  );
}
