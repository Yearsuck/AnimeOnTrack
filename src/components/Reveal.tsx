import { useState } from "react";

/**
 * Grows its children open (grid-template-rows 0fr -> 1fr + fade) when it
 * mounts. The clipping the animation needs is only applied while it runs
 * (`.reveal-anim`), so popovers and focus rings inside are not cut off after.
 */
export function Reveal({ children }: { children: React.ReactNode }) {
  const [animating, setAnimating] = useState(true);
  return (
    <div
      className={`reveal${animating ? " reveal-anim" : ""}`}
      onAnimationEnd={(e) => {
        if (e.target === e.currentTarget) setAnimating(false);
      }}
    >
      <div className="reveal-inner">{children}</div>
    </div>
  );
}
