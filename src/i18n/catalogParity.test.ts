import { describe, expect, it } from "vitest";
import { es } from "./catalog/es";
import { en } from "./catalog/en";
import { ca } from "./catalog/ca";

function extractPlaceholders(text: string): string[] {
  const matches = text.match(/\{(\w+)\}/g);
  if (!matches) return [];
  return Array.from(new Set(matches)).sort();
}

describe("catalog parity", () => {
  const esKeys = Object.keys(es).sort();
  const enKeys = Object.keys(en).sort();
  const caKeys = Object.keys(ca).sort();

  it("every key of the Spanish catalog exists in the English catalog and vice versa", () => {
    const missingInEn = esKeys.filter((k) => !(k in en));
    const extraInEn = enKeys.filter((k) => !(k in es));
    expect(missingInEn, "Keys in es but missing in en").toEqual([]);
    expect(extraInEn, "Keys in en but missing in es").toEqual([]);
    expect(enKeys).toEqual(esKeys);
  });

  it("every key of the Spanish catalog exists in the Catalan catalog and vice versa", () => {
    const missingInCa = esKeys.filter((k) => !(k in ca));
    const extraInCa = caKeys.filter((k) => !(k in es));
    expect(missingInCa, "Keys in es but missing in ca").toEqual([]);
    expect(extraInCa, "Keys in ca but missing in es").toEqual([]);
    expect(caKeys).toEqual(esKeys);
  });

  it("no catalog value is an empty string", () => {
    for (const [key, val] of Object.entries(es)) {
      expect(val, `es["${key}"] must not be empty`).not.toBe("");
      expect(val.trim(), `es["${key}"] must not be only whitespace`).not.toBe("");
    }
    for (const [key, val] of Object.entries(en)) {
      expect(val, `en["${key}"] must not be empty`).not.toBe("");
      expect(val.trim(), `en["${key}"] must not be only whitespace`).not.toBe("");
    }
    for (const [key, val] of Object.entries(ca)) {
      expect(val, `ca["${key}"] must not be empty`).not.toBe("");
      expect(val.trim(), `ca["${key}"] must not be only whitespace`).not.toBe("");
    }
  });

  it("for every key the set of {placeholders} is identical across the three languages", () => {
    const mismatches: Array<{
      key: string;
      es: string[];
      en: string[];
      ca: string[];
    }> = [];

    for (const key of esKeys) {
      const esVal = (es as Record<string, string>)[key] ?? "";
      const enVal = (en as Record<string, string>)[key] ?? "";
      const caVal = (ca as Record<string, string>)[key] ?? "";

      const esPlaceholders = extractPlaceholders(esVal);
      const enPlaceholders = extractPlaceholders(enVal);
      const caPlaceholders = extractPlaceholders(caVal);

      const enMatch = JSON.stringify(esPlaceholders) === JSON.stringify(enPlaceholders);
      const caMatch = JSON.stringify(esPlaceholders) === JSON.stringify(caPlaceholders);

      if (!enMatch || !caMatch) {
        mismatches.push({
          key,
          es: esPlaceholders,
          en: enPlaceholders,
          ca: caPlaceholders,
        });
      }
    }

    expect(mismatches).toEqual([]);
  });
});
