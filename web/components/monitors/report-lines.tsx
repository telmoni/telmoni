import Link from "next/link";

import { CopyButton } from "@/components/copy-button";
import { reportLines } from "@/lib/monitors";

// The ways a monitor's runs are reported, each line to copy as it stands: the
// two pings a crontab's `curl` sends around a job, the CLI wrapping it, and
// an SDK's run naming the agent. The key is the variable, never the key.
export function ReportLines({ slug, agent }: { slug: string; agent: string }) {
  return (
    <div className="grid gap-3">
      <ul className="grid gap-2">
        {reportLines(slug, agent).map(({ label, line }) => (
          <li key={label} className="grid gap-1">
            <span className="text-xs text-muted-foreground">{label}</span>
            <div className="flex min-w-0 items-center gap-1 rounded-md border border-border bg-muted/40 py-1 pr-1 pl-3">
              <code className="min-w-0 flex-1 truncate font-mono text-xs" title={line}>
                {line}
              </code>
              <CopyButton value={line} label={label.toLowerCase()} />
            </div>
          </li>
        ))}
      </ul>
      <p className="text-xs text-muted-foreground">
        <code>TELMONI_PING_KEY</code> is the project&apos;s ping key, from <strong>Ping key</strong>{" "}
        above: kept hashed, it is shown once, when minted. <code>TELMONI_ENDPOINT</code> is this
        deployment&apos;s machine host. An SDK&apos;s run reports under an API key instead;{" "}
        <Link href="/docs/workspace/monitors" className="underline underline-offset-4">
          the docs
        </Link>{" "}
        have the whole of it.
      </p>
    </div>
  );
}
