import type { Episode } from "../types";

/** Returns "download" when the episode is downloaded locally, otherwise "site". */
export function watchTarget(ep: Pick<Episode, "download_status">): "download" | "site" {
  return ep.download_status === "downloaded" ? "download" : "site";
}
