"use client";

import { useState, useTransition } from "react";
import { ChevronDownIcon, KeyRound } from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { LocalTime } from "@/components/local-time";
import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { PingKey } from "@/lib/server/entities/monitors";
import { cn } from "@/lib/utils";

import {
  mintPingKeyAction,
  retirePingKeyAction,
} from "@/app/(app)/[organization]/[project]/monitors/actions";

/** How many retired keys the menu still names, the newest first. */
const RETIRED_SHOWN = 2;

// The project's ping key, on the Monitors page's header for its owners and
// admins: each key by its prefix, when it was made and its last ping; mint
// the first, shown once as an API key is; rotate, a second beside it, the
// older retiring a week after its last ping; and retire the older at once,
// once every crontab line carries the newer.
export function PingKeyMenu({ projectId, keys }: { projectId: string; keys: PingKey[] }) {
  const live = keys.filter((k) => k.retired_at === null);
  const retired = keys.filter((k) => k.retired_at !== null).slice(0, RETIRED_SHOWN);
  const older = live.length > 1 ? live[live.length - 1] : undefined;
  const [minted, setMinted] = useState<string | null>(null);
  const [retiring, setRetiring] = useState<PingKey | null>(null);
  const [pending, start] = useTransition();

  const mint = () =>
    start(async () => {
      const answer = await mintPingKeyAction(projectId);
      if (answer.status === "error") toast.error(answer.error);
      else setMinted(answer.key);
    });

  const retire = (key: PingKey) =>
    start(async () => {
      const answer = await retirePingKeyAction(projectId, key.id);
      setRetiring(null);
      if (answer.error) toast.error(answer.error);
      else toast.success("Key retired: its pings are refused from now on.");
    });

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" disabled={pending}>
            <KeyRound />
            Ping key
            <ChevronDownIcon className="text-muted-foreground" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-80">
          <DropdownMenuLabel>Ping keys</DropdownMenuLabel>
          {live.length === 0 && (
            <p className="px-2 pb-1.5 text-sm text-muted-foreground">
              None yet. A crontab&apos;s pings carry the project&apos;s key in their URL.
            </p>
          )}
          {[...live, ...retired].map((key) => (
            <KeyLine key={key.id} pingKey={key} />
          ))}
          <DropdownMenuSeparator />
          {live.length < 2 ? (
            <DropdownMenuItem onSelect={mint}>
              {live.length === 0 ? "Mint the ping key" : "Rotate: mint a second key"}
            </DropdownMenuItem>
          ) : (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              Two keys are live: the older retires a week after its last ping.
            </p>
          )}
          {older && (
            <DropdownMenuItem variant="destructive" onSelect={() => setRetiring(older)}>
              Retire the older key now
            </DropdownMenuItem>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      {minted && (
        <PlaintextTokenDialog
          token={minted}
          title="Ping key minted"
          description={
            <>
              Copy this key now: <strong>it will not be shown again</strong>. Put it in each
              crontab&apos;s <code>TELMONI_PING_KEY</code>; it can fake a ping, and nothing more.
            </>
          }
          closeLabel="I've saved it"
          onClose={() => setMinted(null)}
        />
      )}
      <ConfirmDialog
        open={retiring !== null}
        onOpenChange={(open) => !open && setRetiring(null)}
        title="Retire the older ping key?"
        description={
          <>
            Every ping that still carries <code>{retiring?.prefix}…</code> is refused from now on.
            Retire it once each crontab line carries the newer key.
          </>
        }
        confirmLabel="Retire key"
        destructive
        onConfirm={() => retiring && retire(retiring)}
      />
    </>
  );
}

/** A key as the menu names it: its prefix, and when it was made, last pinged
 *  or retired. */
function KeyLine({ pingKey }: { pingKey: PingKey }) {
  const retired = pingKey.retired_at !== null;
  return (
    <div className={cn("grid gap-0.5 px-2 py-1.5 text-sm", retired && "text-muted-foreground")}>
      <span className="font-mono text-xs">{pingKey.prefix}…</span>
      <span className="text-xs text-muted-foreground">
        Made <LocalTime iso={pingKey.created_at} mode="date" />
        {" · "}
        {pingKey.retired_at ? (
          <>
            retired <LocalTime iso={pingKey.retired_at} mode="date" />
          </>
        ) : pingKey.last_ping_at ? (
          <>
            last ping <LocalTime iso={pingKey.last_ping_at} mode="datetime" />
          </>
        ) : (
          "no ping yet"
        )}
      </span>
    </div>
  );
}
