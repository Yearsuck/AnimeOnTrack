import { describe, it, expect } from "vitest";
import { watchTarget } from "./watchTarget";

describe("watchTarget", () => {
  it("routes to download when episode is downloaded", () => {
    expect(watchTarget({ download_status: "downloaded" })).toBe("download");
  });

  it("routes to site when download_status is downloading", () => {
    expect(watchTarget({ download_status: "downloading" })).toBe("site");
  });

  it("routes to site when download_status is failed", () => {
    expect(watchTarget({ download_status: "failed" })).toBe("site");
  });

  it("routes to site when download_status is null", () => {
    expect(watchTarget({ download_status: null })).toBe("site");
  });

  it("routes to site when download_status is undefined", () => {
    expect(watchTarget({ download_status: undefined })).toBe("site");
    expect(watchTarget({})).toBe("site");
  });
});
