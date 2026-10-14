import { describe, it, expect } from "vitest";
import { isDownloadableUrl, summarizeDownloads } from "./downloadSummary";

describe("isDownloadableUrl", () => {
  it("recognizes every supported site and its subdomains case-insensitively", () => {
    expect(isDownloadableUrl("https://jkanime.net/kanojo/2/")).toBe(true);
    expect(isDownloadableUrl("https://animeyt.cc/1/anime/x-capitulo-8/")).toBe(true);
    expect(isDownloadableUrl("https://gogoanime.by/x-episode-1-english-subbed/")).toBe(true);
    expect(isDownloadableUrl("https://tioanime.com/ver/x-1")).toBe(true);
    expect(isDownloadableUrl("https://www.tioanime.com/ver/x")).toBe(true);
    expect(isDownloadableUrl("https://TIOANIME.COM/x")).toBe(true);
  });

  it("rejects other domains, deceptive hosts, and invalid URLs without throwing", () => {
    expect(isDownloadableUrl("https://www4.animeflv.net/ver/x-1")).toBe(false);
    expect(isDownloadableUrl("https://notjkanime.net/x")).toBe(false);
    expect(isDownloadableUrl("https://nottioanime.com/x")).toBe(false);
    expect(isDownloadableUrl("https://tioanime.com.evil.example/x")).toBe(false);
    expect(isDownloadableUrl("not a url")).toBe(false);
    expect(isDownloadableUrl("")).toBe(false);
  });
});

describe("summarizeDownloads", () => {
  it("summarizes the 6 test episodes accurately", () => {
    const episodes = [
      { url: "https://tioanime.com/ver/x-1", seen: false, download_status: null },
      { url: "https://tioanime.com/ver/x-2", seen: false, download_status: "failed" as const },
      { url: "https://tioanime.com/ver/x-3", seen: true, download_status: null },
      { url: "https://tioanime.com/ver/x-4", seen: false, download_status: "downloaded" as const },
      { url: "https://tioanime.com/ver/x-5", seen: false, download_status: "downloading" as const },
      { url: "https://www4.animeflv.net/ver/x-6", seen: false, download_status: null },
    ];

    expect(summarizeDownloads(episodes)).toEqual({
      downloadable: 5,
      downloaded: 1,
      active: 1,
      failed: 1,
      pending: 2,
    });
  });

  it("returns all zeros for an empty list", () => {
    expect(summarizeDownloads([])).toEqual({
      downloadable: 0,
      downloaded: 0,
      active: 0,
      failed: 0,
      pending: 0,
    });
  });
});
