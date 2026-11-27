import { useCallback, useRef, useState } from "react";
import { applyPatch, rollbackPatch } from "./settingSave";

/// Optimistic settings state: `save(patch, persist)` shows the new value at
/// once, and if `persist` rejects it rolls the value back and sets `failed`
/// so the card can show an inline, recoverable error. Saves run one at a time
/// (`saving` is true meanwhile and a second `save` is ignored): the persisted
/// payload is the whole state, so overlapping saves could leave the UI and the
/// backend disagreeing after a rollback.
export function useSettingSave<T extends object>(initial: T) {
  const [value, setValue] = useState<T>(initial);
  const [failed, setFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const ref = useRef(value);
  const inFlight = useRef(false);

  const commit = (next: T) => {
    ref.current = next;
    setValue(next);
  };

  const replace = useCallback((next: T) => {
    ref.current = next;
    setValue(next);
  }, []);

  const save = useCallback(
    async (patch: Partial<T>, persist: (next: T) => Promise<unknown>) => {
      if (inFlight.current) return;
      inFlight.current = true;
      setSaving(true);
      const prev = ref.current;
      const next = applyPatch(prev, patch);
      setFailed(false);
      commit(next);
      try {
        await persist(next);
      } catch (err) {
        console.error("Failed to save setting:", err);
        commit(rollbackPatch(ref.current, prev, patch));
        setFailed(true);
      } finally {
        inFlight.current = false;
        setSaving(false);
      }
    },
    [],
  );

  const clearError = useCallback(() => setFailed(false), []);
  return { value, replace, save, failed, saving, clearError };
}
