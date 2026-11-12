import type { ServiceState } from "@/lib/footer";
import { StatusIndicator } from "@/components/status-badge";

/** Service health stays named alongside its indicators. */
export function StatusLabel({ label, services }: { label: string; services?: readonly ServiceState[] }) {
  if (!services?.length) return <>{label}</>;
  const down = services.filter((service) => service.state === "disrupted").map((service) => service.name);
  return (
    <>
      <span aria-hidden="true" className="mr-1.5 inline-flex h-icon items-center gap-1.5 align-middle">
        {services.map((service) => (
          <StatusIndicator
            key={service.name}
            live={service.state === "disrupted"}
            className={service.state === "normal" ? "bg-brand-positive" : "bg-brand-negative"}
          />
        ))}
      </span>
      <span className="sr-only">{label}: </span>
      {down.length === 0 ? "Normal" : "Disrupted"}
      {down.length > 0 && <span className="sr-only"> ({down.join(", ")})</span>}
    </>
  );
}
