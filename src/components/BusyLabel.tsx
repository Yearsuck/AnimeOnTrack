interface BusyLabelProps {
  busy: boolean;
  idle: string;
  busyText: string;
}

/**
 * Button content that swaps to a spinner + "…ing" label while busy. Both labels sit
 * in the same grid cell, so the button is always as wide as the wider one and never
 * changes width between states; the inactive one is transparent and aria-hidden.
 * Put aria-busy={busy} on the button itself.
 */
export function BusyLabel({ busy, idle, busyText }: BusyLabelProps) {
  return (
    <span className="busy-label" data-busy={busy ? "" : undefined}>
      <span className="busy-label-idle" aria-hidden={busy || undefined}>
        {idle}
      </span>
      <span className="busy-label-busy" aria-hidden={!busy || undefined}>
        <span className="spinner" aria-hidden="true" />
        {busyText}
      </span>
    </span>
  );
}
