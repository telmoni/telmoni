// @vitest-environment jsdom
import { renderHook, act } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { CORNERS_BOOT_SCRIPT, SHARP_CLASS } from "@/lib/corners";
import { bootCorners, setCorners, useCorners } from "@/lib/use-corners";

afterEach(() => {
  document.documentElement.classList.remove(SHARP_CLASS);
  window.localStorage.clear();
});

/** What the `<head>` script does on a cold load, run against jsdom. */
function boot() {
  new Function(CORNERS_BOOT_SCRIPT)();
}

describe("the corner choice survives leaving and coming back", () => {
  it("is restored by the boot script from a previous visit", () => {
    window.localStorage.setItem("telmoni-corners", "sharp");

    boot();

    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(true);
    const { result } = renderHook(() => useCorners());
    expect(result.current).toBe("sharp");
  });

  it("leaves a first-time visitor rounded", () => {
    boot();
    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(
      false,
    );
    expect(renderHook(() => useCorners()).result.current).toBe("rounded");
  });

  // ⚠ The boot script is a STRING, so a typo in it compiles and ships. Running
  // the real constant is the only thing that proves the key it reads is the
  // key `setCorners` writes.
  it("reads the key the setter writes, not a second spelling of it", () => {
    renderHook(() => useCorners());
    act(() => setCorners("sharp"));
    document.documentElement.classList.remove(SHARP_CLASS);

    boot();

    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(true);
  });

  it("goes back to rounded and clears the class", () => {
    const { result } = renderHook(() => useCorners());
    act(() => setCorners("sharp"));
    expect(result.current).toBe("sharp");

    act(() => setCorners("rounded"));

    expect(result.current).toBe("rounded");
    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(
      false,
    );
    expect(window.localStorage.getItem("telmoni-corners")).toBe("rounded");
  });
});

describe("a second tab", () => {
  function fromOtherTab(newValue: string | null) {
    window.dispatchEvent(
      new StorageEvent("storage", { key: "telmoni-corners", newValue }),
    );
  }

  it("changes this one, the way next-themes does for light and dark", () => {
    const { result } = renderHook(() => useCorners());

    act(() => fromOtherTab("sharp"));

    expect(result.current).toBe("sharp");
    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(true);
  });

  it("falls back to rounded on a value this build does not know", () => {
    const { result } = renderHook(() => useCorners());
    act(() => setCorners("sharp"));

    act(() => fromOtherTab("squircle"));

    expect(result.current).toBe("rounded");
  });

  it("is ignored when some other key changed", () => {
    const { result } = renderHook(() => useCorners());
    act(() => setCorners("sharp"));

    act(() =>
      window.dispatchEvent(
        new StorageEvent("storage", { key: "theme", newValue: "dark" }),
      ),
    );

    expect(result.current).toBe("sharp");
  });
});

// A 404 Next answers from a layout's `notFound()` is rendered from scratch in
// the browser, where the `<head>` script never runs: `bootCorners` is its
// stand-in there, and must do exactly what it does.
describe("a page the boot script never ran on", () => {
  it("restores the saved choice as the script would, and tells the hook", () => {
    window.localStorage.setItem("telmoni-corners", "sharp");
    const { result } = renderHook(() => useCorners());
    expect(result.current).toBe("rounded");

    act(() => bootCorners());

    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(true);
    expect(result.current).toBe("sharp");
  });

  it("agrees with the boot script for every value storage may hold", () => {
    const after = (saved: string | null, run: () => void) => {
      document.documentElement.classList.remove(SHARP_CLASS);
      window.localStorage.clear();
      if (saved !== null) window.localStorage.setItem("telmoni-corners", saved);
      run();
      return document.documentElement.classList.contains(SHARP_CLASS);
    };
    for (const saved of ["sharp", "rounded", "nonsense", null]) {
      expect(after(saved, bootCorners), String(saved)).toBe(after(saved, boot));
    }
  });

  it("changes nothing on a page the script ran on", () => {
    window.localStorage.setItem("telmoni-corners", "sharp");
    boot();
    const { result } = renderHook(() => useCorners());
    act(() => bootCorners());
    expect(document.documentElement.classList.contains(SHARP_CLASS)).toBe(true);
    expect(result.current).toBe("sharp");
  });
});
