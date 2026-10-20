// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";

import { FragmentEntries } from "@/components/fragment-entries";

const replace = vi.fn();

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace }),
  usePathname: () => "/acme/web/traces",
  useSearchParams: () => new URLSearchParams("days=30"),
}));

const traversal = (state: unknown) => window.dispatchEvent(new PopStateEvent("popstate", { state }));

describe("a traversal onto an entry a fragment made", () => {
  let rewritten: MockInstance<History["replaceState"]>;
  beforeEach(() => {
    replace.mockClear();
    window.history.replaceState(null, "", "/acme/web/traces?days=30#typed");
    rewritten = vi.spyOn(window.history, "replaceState");
  });
  afterEach(() => rewritten.mockRestore());

  it("is handed to the router in place where the page on show is the entry's, its fragment kept", () => {
    render(<FragmentEntries />);
    traversal(null);
    expect(rewritten).toHaveBeenCalledExactlyOnceWith({}, "", "/acme/web/traces?days=30#typed");
    expect(replace).not.toHaveBeenCalled();
  });

  it("has another page or query read through the router, its fragment kept and no scroll asked", () => {
    render(<FragmentEntries />);
    window.history.pushState(null, "", "/acme/web/traces?days=7#typed");
    traversal(null);
    expect(replace).toHaveBeenCalledExactlyOnceWith("/acme/web/traces?days=7#typed", { scroll: false });
    expect(rewritten).not.toHaveBeenCalled();
  });

  it("is left to the router where the entry is one of its own", () => {
    render(<FragmentEntries />);
    traversal({ __NA: true });
    expect(replace).not.toHaveBeenCalled();
    expect(rewritten).not.toHaveBeenCalled();
  });

  it("is let go of once the page is gone", () => {
    render(<FragmentEntries />).unmount();
    traversal(null);
    expect(replace).not.toHaveBeenCalled();
    expect(rewritten).not.toHaveBeenCalled();
  });
});
