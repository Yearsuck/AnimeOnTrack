import { useT } from "../../../i18n";

export interface ListasErrorProps {
  error: string;
  onRetry: () => void;
}

export function ListasError({ error, onRetry }: ListasErrorProps) {
  const t = useT();

  return (
    <div className="listas-error-box" role="alert">
      <div>
        <strong>{t("discover.listas.loadErrorTitle")}:</strong> {error}
      </div>
      <button type="button" className="btn btn-sm btn-ghost" onClick={onRetry}>
        {t("discover.listas.retry")}
      </button>
    </div>
  );
}
