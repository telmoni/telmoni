import type { ServiceState } from "@/lib/footer";
import { cn } from "@/lib/utils";

/** A footer link's label, or, for a status page that names its services, a
 *  bar for each, green or red, and what they come to: Normal or Disrupted. */
export function StatusLabel({ label, services }: { label: string; services?: readonly ServiceState[] }) {
  if (!services?.length) return <>{label}</>;
  const down = services.filter((service) => service.state === "disrupted").map((service) => service.name);
  return (
    <>
      <span aria-hidden="true" className="mr-1.5 inline-flex h-icon gap-0.5 align-middle">
        {services.map((service) => (
          <span
            key={service.name}
            className={cn("w-1 rounded-xs", service.state === "normal" ? "bg-brand-positive" : "bg-brand-negative")}
          />
        ))}
      </span>
      <span className="sr-only">{label}: </span>
      {down.length === 0 ? "Normal" : "Disrupted"}
      {down.length > 0 && <span className="sr-only"> ({down.join(", ")})</span>}
    </>
  );
}
