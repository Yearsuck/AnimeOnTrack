export type LinkStatus = {
  id: number;
  title: string;
  state: "searching" | "linked" | "nomatch";
  episodes?: number;
};

export type SubView = "swipe" | "listas";

export type SwipeOutDirection = "discard" | "want" | "seen" | null;

export type ListTab = "want" | "discarded" | "watched";
