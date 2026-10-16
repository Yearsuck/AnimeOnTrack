import { describe, it, expect } from "vitest";
import { formatBytes } from "./formatBytes";

describe("formatBytes", () => {
  it("formats 0 bytes in en as '0 B'", () => {
    expect(formatBytes(0, "en")).toBe("0 B");
  });

  it("formats 512 bytes in en as '512 B'", () => {
    expect(formatBytes(512, "en")).toBe("512 B");
  });

  it("formats 1536 bytes in en as '1.5 KB'", () => {
    expect(formatBytes(1536, "en")).toBe("1.5 KB");
  });

  it("formats 30 MB in en as '30 MB'", () => {
    expect(formatBytes(30 * 1024 ** 2, "en")).toBe("30 MB");
  });

  it("formats 5.08 GB in en as '5.08 GB'", () => {
    expect(formatBytes(5.08 * 1024 ** 3, "en")).toBe("5.08 GB");
  });

  it("formats 1536 bytes in es using a decimal comma ('1,5 KB')", () => {
    expect(formatBytes(1536, "es")).toBe("1,5 KB");
  });

  it("formats 1536 bytes in ca using a decimal comma ('1,5 KB')", () => {
    expect(formatBytes(1536, "ca")).toBe("1,5 KB");
  });

  it("handles negative and non-finite values gracefully", () => {
    expect(formatBytes(-100, "en")).toBe("0 B");
    expect(formatBytes(NaN, "en")).toBe("0 B");
    expect(formatBytes(Infinity, "en")).toBe("0 B");
  });

  it("formats exact powers of 1024", () => {
    expect(formatBytes(1, "en")).toBe("1 B");
    expect(formatBytes(1024, "en")).toBe("1 KB");
    expect(formatBytes(1024 ** 2, "en")).toBe("1 MB");
    expect(formatBytes(1024 ** 3, "en")).toBe("1 GB");
    expect(formatBytes(2 * 1024 ** 4, "en")).toBe("2 TB");
  });
});
