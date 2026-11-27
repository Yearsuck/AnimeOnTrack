import type { ReactNode } from "react";

interface SettingsRowProps {
  /** Label content. */
  label: ReactNode;
  /** Renders the label as `<span id>`; use for groups of buttons/inputs. */
  labelId?: string;
  /** Renders the label as `<label htmlFor>`; use for a single form control. */
  htmlFor?: string;
  /** Also marks the whole row `role="group"` labelled by `labelId`. */
  group?: boolean;
  /** Control(s), hint and any extra content, rendered after the label. */
  children?: ReactNode;
}

/** Settings field: `.settings-field` wrapper + `.settings-field-label` + content. */
export function SettingsRow({ label, labelId, htmlFor, group, children }: SettingsRowProps) {
  return (
    <div
      className="settings-field"
      {...(group && labelId ? { role: "group", "aria-labelledby": labelId } : {})}
    >
      {htmlFor ? (
        <label className="settings-field-label" htmlFor={htmlFor}>
          {label}
        </label>
      ) : (
        <span className="settings-field-label" id={labelId}>
          {label}
        </span>
      )}
      {children}
    </div>
  );
}
