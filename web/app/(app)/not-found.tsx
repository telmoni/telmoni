import Link from "next/link";

export default function ConsoleNotFound() {
  return (
    <div className="flex flex-col gap-4 max-w-md py-8">
      <p className="text-xs tracking-label text-muted-foreground uppercase">404</p>
      <h1 className="text-xl font-light tracking-tight">Not found.</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        Nothing at this address. An organization&apos;s and a project&apos;s
        addresses follow their names: this one may have been renamed, moved
        or deleted since the link was made, or it isn&apos;t one you&apos;re
        in. Find it in the organization menu.
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
