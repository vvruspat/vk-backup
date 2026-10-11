import { describe, expect, it } from "vitest";
import { formatBytes, isBusy, percent, type Progress } from "./types";

describe("progress", () => {
  it("counts unavailable files and already archived files without exceeding 100 percent", () => {
    const p = {
      total: 10,
      completed: 5,
      skipped: 3,
      failed: 1,
      unavailable: 1,
    } as Progress;
    expect(percent(p)).toBe(100);
    expect(percent({ ...p, total: 0 })).toBe(0);
    expect(percent({ ...p, failed: 8 })).toBe(100);
  });
  it("canceling keeps account and source changes locked", () => {
    expect(isBusy({ status: "canceling" } as Progress)).toBe(true);
    expect(isBusy({ status: "partial" } as Progress)).toBe(false);
    expect(formatBytes(1024)).toBe("1 КБ");
  });
});
