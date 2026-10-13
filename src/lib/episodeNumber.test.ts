import { describe, expect, it } from "vitest";
import { epNum } from "./episodeNumber";

describe("epNum", () => {
  it("parses '1x13' as season 1 episode 13 (100013)", () => {
    expect(epNum("1x13")).toBe(100013);
  });

  it("parses '1x05' as 100005", () => {
    expect(epNum("1x05")).toBe(100005);
    expect(epNum("1x05") === 100005).toBe(true);
  });

  it("parses '2x03' as 200003", () => {
    expect(epNum("2x03")).toBe(200003);
  });

  it("handles whitespace padding: ' 5 ' -> 5", () => {
    expect(epNum(" 5 ")).toBe(5);
  });

  it("falls back to leading digits when secondary half is non-numeric: '1xx5' -> 1", () => {
    expect(epNum("1xx5")).toBe(1);
  });

  it("returns null when SxE has no season prefix: 'x5' -> null", () => {
    expect(epNum("x5")).toBeNull();
  });

  it("returns null for empty string: '' -> null", () => {
    expect(epNum("")).toBeNull();
  });

  it("parses standard integer episode number: '12' -> 12", () => {
    expect(epNum("12")).toBe(12);
  });

  it("parses decimal episode number: '12.5' -> 12.5", () => {
    expect(epNum("12.5")).toBe(12.5);
  });

  it("drops trailing dot: '12.' -> 12", () => {
    expect(epNum("12.")).toBe(12);
  });

  it("stops at second dot: '1.2.3' -> 1.2", () => {
    expect(epNum("1.2.3")).toBe(1.2);
  });

  it("parses leading zero episode with title suffix: '0 | Recap' -> 0", () => {
    expect(epNum("0 | Recap")).toBe(0);
  });

  it("returns null for non-numeric episode label: 'OVA' -> null", () => {
    expect(epNum("OVA")).toBeNull();
  });

  it("orders '13.5' > '13' for cascade comparison", () => {
    const ep13_5 = epNum("13.5");
    const ep13 = epNum("13");
    expect(ep13_5).not.toBeNull();
    expect(ep13).not.toBeNull();
    expect(ep13_5!).toBeGreaterThan(ep13!);
  });

  it("orders '1x01' > '12' for cascade comparison across seasons", () => {
    const ep1x01 = epNum("1x01");
    const ep12 = epNum("12");
    expect(ep1x01).not.toBeNull();
    expect(ep12).not.toBeNull();
    expect(ep1x01!).toBeGreaterThan(ep12!);
  });
});
