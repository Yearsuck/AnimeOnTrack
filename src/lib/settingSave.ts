/// Pure helpers for optimistic settings saves with rollback.

/// Applies `patch` on top of `state` (the optimistic value).
export function applyPatch<T extends object>(state: T, patch: Partial<T>): T {
  return { ...state, ...patch };
}

/// Restores only the keys of `patch` to their values in `prev`, leaving any
/// other key (changed meanwhile by another save) untouched.
export function rollbackPatch<T extends object>(
  state: T,
  prev: T,
  patch: Partial<T>,
): T {
  const next = { ...state };
  for (const key of Object.keys(patch) as (keyof T)[]) {
    next[key] = prev[key];
  }
  return next;
}
