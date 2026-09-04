// Catalog.tsx and Library.tsx each independently reimplemented the same
// "persist filter state in a module-level variable across unmount/remount"
// idiom (a tab switch in App.tsx unmounts the view; a plain `useState`
// default would silently reset every filter back to empty on return). One
// small factory here instead of a hand-rolled `let X: Shape | null = null`
// in each view file.
export function createFilterCache<T>() {
  let value: T | null = null;
  return {
    get current(): T | null {
      return value;
    },
    write(next: T) {
      value = next;
    },
  };
}
