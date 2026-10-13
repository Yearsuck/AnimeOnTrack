import { describe, expect, it } from "vitest";
import { shortcutFor, SHORTCUT_ROWS } from "./shortcuts";

function createEvent(overrides: Partial<Parameters<typeof shortcutFor>[0]> = {}) {
  return {
    key: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    targetIsEditable: false,
    ...overrides,
  };
}

describe("shortcutFor", () => {
  it("maps digit '1' to pending", () => {
    expect(shortcutFor(createEvent({ key: "1" }))).toEqual({
      kind: "navigate",
      view: "pending",
    });
  });

  it("maps digit '2' to airing", () => {
    expect(shortcutFor(createEvent({ key: "2" }))).toEqual({
      kind: "navigate",
      view: "airing",
    });
  });

  it("maps digit '3' to library", () => {
    expect(shortcutFor(createEvent({ key: "3" }))).toEqual({
      kind: "navigate",
      view: "library",
    });
  });

  it("maps digit '4' to descubrir", () => {
    expect(shortcutFor(createEvent({ key: "4" }))).toEqual({
      kind: "navigate",
      view: "descubrir",
    });
  });

  it("maps digit '5' to catalog", () => {
    expect(shortcutFor(createEvent({ key: "5" }))).toEqual({
      kind: "navigate",
      view: "catalog",
    });
  });

  it("maps digit '6' to stats", () => {
    expect(shortcutFor(createEvent({ key: "6" }))).toEqual({
      kind: "navigate",
      view: "stats",
    });
  });

  it("maps digit '7' to settings", () => {
    expect(shortcutFor(createEvent({ key: "7" }))).toEqual({
      kind: "navigate",
      view: "settings",
    });
  });

  it("maps 'r' to refresh", () => {
    expect(shortcutFor(createEvent({ key: "r" }))).toEqual({
      kind: "refresh",
    });
  });

  it("maps uppercase 'R' without shift to refresh (Caps Lock)", () => {
    expect(shortcutFor(createEvent({ key: "R" }))).toEqual({
      kind: "refresh",
    });
  });

  it("maps '/' to search", () => {
    expect(shortcutFor(createEvent({ key: "/" }))).toEqual({
      kind: "search",
    });
  });

  it("maps '?' to help without shift", () => {
    expect(shortcutFor(createEvent({ key: "?", shiftKey: false }))).toEqual({
      kind: "help",
    });
  });

  it("maps '?' to help with shiftKey allowed", () => {
    expect(shortcutFor(createEvent({ key: "?", shiftKey: true }))).toEqual({
      kind: "help",
    });
  });

  it("maps 'Escape' to escape", () => {
    expect(shortcutFor(createEvent({ key: "Escape" }))).toEqual({
      kind: "escape",
    });
  });

  it("returns null for all shortcuts when targetIsEditable is true", () => {
    const keys = ["1", "2", "3", "4", "5", "6", "7", "r", "R", "/", "?", "Escape"];
    for (const key of keys) {
      expect(
        shortcutFor(
          createEvent({
            key,
            targetIsEditable: true,
            shiftKey: key === "?",
          })
        )
      ).toBeNull();
    }
  });

  it("returns null when ctrlKey is pressed (e.g. ctrl+1)", () => {
    expect(shortcutFor(createEvent({ key: "1", ctrlKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "r", ctrlKey: true }))).toBeNull();
  });

  it("returns null when altKey is pressed (e.g. alt+r)", () => {
    expect(shortcutFor(createEvent({ key: "r", altKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "1", altKey: true }))).toBeNull();
  });

  it("returns null when metaKey is pressed", () => {
    expect(shortcutFor(createEvent({ key: "1", metaKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "r", metaKey: true }))).toBeNull();
  });

  it("returns null for unknown key 'a'", () => {
    expect(shortcutFor(createEvent({ key: "a" }))).toBeNull();
  });

  it("returns null for digits out of range '8' and '0'", () => {
    expect(shortcutFor(createEvent({ key: "8" }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "0" }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "9" }))).toBeNull();
  });

  it("returns null for shift+'1' (a '!' key)", () => {
    expect(shortcutFor(createEvent({ key: "1", shiftKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "!", shiftKey: true }))).toBeNull();
  });

  it("returns null when shiftKey is held for keys other than '?'", () => {
    expect(shortcutFor(createEvent({ key: "r", shiftKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "R", shiftKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "/", shiftKey: true }))).toBeNull();
    expect(shortcutFor(createEvent({ key: "Escape", shiftKey: true }))).toBeNull();
  });

  it("exports SHORTCUT_ROWS with expected shortcuts and labelKeys", () => {
    expect(SHORTCUT_ROWS).toHaveLength(5);
    expect(SHORTCUT_ROWS.map((r) => r.labelKey)).toEqual([
      "shortcuts.nav",
      "shortcuts.refresh",
      "shortcuts.search",
      "shortcuts.help",
      "shortcuts.escape",
    ]);
  });
});
