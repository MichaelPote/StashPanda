/** RFC 7396 JSON merge patch helpers used by the settings and UI-state stores. */

export type JsonObject = { [key: string]: unknown };

function isPlainObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Applies `patch` to `target` the way the Rust side will, returning a new object. */
export function applyMergePatch<T>(target: T, patch: JsonObject): T {
  const base: JsonObject = isPlainObject(target) ? { ...target } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    if (value === null) {
      delete base[key];
    } else if (isPlainObject(value)) {
      base[key] = applyMergePatch(base[key], value);
    } else {
      base[key] = value;
    }
  }
  return base as T;
}

/** Combines two patches into one that has the same effect as applying `first` then `second`. */
export function combinePatches(first: JsonObject, second: JsonObject): JsonObject {
  const result: JsonObject = { ...first };
  for (const [key, value] of Object.entries(second)) {
    if (value === undefined) continue;
    const existing = result[key];
    if (isPlainObject(existing) && isPlainObject(value)) {
      result[key] = combinePatches(existing, value);
    } else {
      result[key] = value;
    }
  }
  return result;
}
