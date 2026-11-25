import { afterEach, describe, expect, it, vi } from "vitest";
import { onPageLeave } from "./ControlPad";

describe("held input", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("is let go when the page loses focus or is hidden, as the finger's release may never come", () => {
    const win = new EventTarget();
    const doc = Object.assign(new EventTarget(), { visibilityState: "visible" });
    vi.stubGlobal("window", win);
    vi.stubGlobal("document", doc);
    const leave = vi.fn();
    const stop = onPageLeave(leave);
    win.dispatchEvent(new Event("blur"));
    win.dispatchEvent(new Event("pagehide"));
    doc.dispatchEvent(new Event("visibilitychange"));
    expect(leave).toHaveBeenCalledTimes(2);
    doc.visibilityState = "hidden";
    doc.dispatchEvent(new Event("visibilitychange"));
    expect(leave).toHaveBeenCalledTimes(3);
    stop();
    win.dispatchEvent(new Event("blur"));
    expect(leave).toHaveBeenCalledTimes(3);
  });
});
