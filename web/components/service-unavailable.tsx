import { RetryButton } from "@/components/retry-button";

export function ServiceUnavailable({
  title = "This page is temporarily unavailable.",
  detail = "This is usually brief and your data is safe. Try again in a moment.",
  retry = true,
}: {
  title?: string;
  detail?: string;
  retry?: boolean;
}) {
  return (
    <div className="flex flex-col items-start gap-4 max-w-md py-8" role="alert">
      <p className="text-xs tracking-label text-brand-negative uppercase">unavailable</p>
      <h1 className="text-xl font-light tracking-tight">{title}</h1>
      <p className="text-sm text-muted-foreground leading-relaxed">{detail}</p>
      {retry && <RetryButton />}
    </div>
  );
}
