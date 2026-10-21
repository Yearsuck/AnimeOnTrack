import { describe, expect, it } from "vitest";
import { es } from "../i18n/catalog/es";
import { en } from "../i18n/catalog/en";
import { ca } from "../i18n/catalog/ca";
import {
  TUTORIAL_SEEN_KEY,
  TUTORIAL_STEPS,
  clampStep,
  readTutorialSeen,
  shouldAutoShowTutorial,
  writeTutorialSeen,
  type KeyValueStore,
} from "./tutorial";

function memoryStore(): KeyValueStore & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return { data, getItem: (k) => data.get(k) ?? null, setItem: (k, v) => void data.set(k, v) };
}

const throwingStore: KeyValueStore = {
  getItem() {
    throw new Error("blocked");
  },
  setItem() {
    throw new Error("blocked");
  },
};

describe("TUTORIAL_STEPS", () => {
  it("has unique ids and 6 steps", () => {
    const ids = TUTORIAL_STEPS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.length).toBe(6);
  });

  it("has its i18n keys in es, en and ca", () => {
    for (const step of TUTORIAL_STEPS) {
      for (const key of [step.titleKey, step.bodyKey]) {
        for (const cat of [es, en, ca] as Record<string, string>[]) {
          expect(cat[key], key).toBeTruthy();
        }
      }
    }
  });
});

describe("tutorial persistence", () => {
  it("round-trips the marker", () => {
    const s = memoryStore();
    expect(readTutorialSeen(s)).toBe(false);
    writeTutorialSeen(s);
    expect(s.data.get(TUTORIAL_SEEN_KEY)).toBe("1");
    expect(readTutorialSeen(s)).toBe(true);
  });

  it("survives a failing or missing storage", () => {
    expect(readTutorialSeen(throwingStore)).toBe(false);
    expect(() => writeTutorialSeen(throwingStore)).not.toThrow();
    expect(readTutorialSeen(null)).toBe(false);
    expect(() => writeTutorialSeen(null)).not.toThrow();
  });
});

describe("shouldAutoShowTutorial", () => {
  it("shows to anyone who has not seen it, existing users included", () => {
    expect(shouldAutoShowTutorial({ seen: false })).toBe(true);
  });
  it("never shows twice", () => {
    expect(shouldAutoShowTutorial({ seen: true })).toBe(false);
  });
});

describe("clampStep", () => {
  it("stays in range", () => {
    expect(clampStep(-3)).toBe(0);
    expect(clampStep(99)).toBe(TUTORIAL_STEPS.length - 1);
  });
});
