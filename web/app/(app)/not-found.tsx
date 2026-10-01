import Link from "next/link";

export default function ConsoleNotFound() {
  return (
    <div className="flex flex-col gap-4 max-w-md py-8">
      <p className="text-xs tracking-label text-muted-foreground uppercase">404</p>
      <h1 className="text-xl font-light tracking-tight">Not found.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        Nothing at this address in the organization you&apos;re working in. A
        project belongs to one organization: if this one was shared with you
        elsewhere, switch to that organization from the organization menu and open
        it from there.
      </p>
      <div>
        <Link
          href="/console"
          className="text-sm rounded-menu border border-border px-4 py-2.5 hover:bg-secondary transition-colors"
        >
          Back to console
        </Link>
      </div>
    </div>
  );
}
