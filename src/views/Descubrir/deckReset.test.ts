import { describe, expect, it } from "vitest";
import { countActiveFilters, DECK_DEFAULTS } from "./deckReset";

describe("deckReset", () => {
  describe("DECK_DEFAULTS", () => {
    it("has empty arrays, false, all null, and nicheBias 0", () => {
      expect(DECK_DEFAULTS.genres).toEqual([]);
      expect(DECK_DEFAULTS.formats).toEqual([]);
      expect(DECK_DEFAULTS.hideUpcoming).toBe(false);
      expect(DECK_DEFAULTS.minStartDate).toBeNull();
      expect(DECK_DEFAULTS.maxStartDate).toBeNull();
      expect(DECK_DEFAULTS.minScore).toBeNull();
      expect(DECK_DEFAULTS.nicheBias).toBe(0);
    });
  });

  describe("countActiveFilters", () => {
    it("returns 0 with everything empty, null, or false", () => {
      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: false,
          minStartDate: null,
          maxStartDate: null,
          minScore: null,
        })
      ).toBe(0);
    });

    it("matches the must-pass example with 6 active filters", () => {
      expect(
        countActiveFilters({
          genres: ["a", "b"],
          formats: ["TV"],
          hideUpcoming: true,
          minStartDate: 2000,
          maxStartDate: null,
          minScore: 70,
        })
      ).toBe(6);
    });

    it("accepts a Set as well as an array for genres and formats", () => {
      expect(
        countActiveFilters({
          genres: new Set(["Action", "Comedy", "Sci-Fi"]),
          formats: new Set(["TV", "Movie"]),
          hideUpcoming: false,
          minStartDate: null,
          maxStartDate: null,
          minScore: null,
        })
      ).toBe(5);
    });

    it("counts both date bounds set as ONE filter", () => {
      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: false,
          minStartDate: 2010,
          maxStartDate: 2024,
          minScore: null,
        })
      ).toBe(1);
    });

    it("counts only minStartDate or only maxStartDate as ONE filter", () => {
      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: false,
          minStartDate: 2010,
          maxStartDate: null,
          minScore: null,
        })
      ).toBe(1);

      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: false,
          minStartDate: null,
          maxStartDate: 2024,
          minScore: null,
        })
      ).toBe(1);
    });

    it("counts score and hideUpcoming independently", () => {
      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: true,
          minStartDate: null,
          maxStartDate: null,
          minScore: null,
        })
      ).toBe(1);

      expect(
        countActiveFilters({
          genres: [],
          formats: [],
          hideUpcoming: false,
          minStartDate: null,
          maxStartDate: null,
          minScore: 80,
        })
      ).toBe(1);
    });
  });
});
