import Link from "next/link";

// `data-console-not-found` is what the rail reads (`console-sidebar.tsx`): the
// rows it would draw for this address all lead back here.
export default function ConsoleNotFound() {
  return (
    <div data-console-not-found className="flex flex-col gap-4 max-w-md py-8">
      <p className="text-xs tracking-label text-muted-foreground uppercase">404</p>
      <h1 className="text-xl font-light tracking-tight">Not found.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        Nothing at this address. A project&apos;s address follows its name and
        an organization&apos;s URL is a setting of its own: this one may have
        moved or been deleted since the link was made, or it isn&apos;t one
        you&apos;re in. Find it in the organization menu.
      </p>
      <div>
        <Link
          href="/console"
          className="text-sm rounded-md border border-border px-4 py-2.5 hover:bg-secondary transition-colors"
        >
          Back to console
        </Link>
      </div>
    </div>
  );
}
