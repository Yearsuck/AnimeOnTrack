import type { ButtonHTMLAttributes } from "react";

interface ChipProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** Adds the `active` class (selected state). */
  active?: boolean;
}

/**
 * Toggle chip: a `<button type="button" class="chip-toggle [extra] [active]">`.
 * Everything else (aria-pressed, disabled, title, onClick...) passes through.
 */
export function Chip({ active, className, children, ...rest }: ChipProps) {
  return (
    <button
      type="button"
      className={`chip-toggle${className ? ` ${className}` : ""}${active ? " active" : ""}`}
      {...rest}
    >
      {children}
    </button>
  );
}
