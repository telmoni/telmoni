// Footer links a console built on this one adds: empty here, and the file
// such a console lays its own copy over. Each joins the column it names —
// one the core draws, or a new one, added after the core's — before the link
// it names, or last. So the core never names a page it does not have.
export interface ExtraFooterLink {
  column: string;
  label: string;
  href: string;
  newTab?: boolean;
  /** The link it goes before, by label; last in its column when absent. */
  before?: string;
  /** What a status page it links says of each service now, a bar each. */
  services?: readonly { name: string; state: "normal" | "disrupted" }[];
}

export const EXTRA_FOOTER_LINKS: readonly ExtraFooterLink[] = [];
