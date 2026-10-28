/** Whether `value` is one of `values`: every closed set's guard, without a
 *  cast. */
export function isOneOf<T extends string | number>(values: readonly T[], value: unknown): value is T {
  const all: readonly unknown[] = values;
  return all.includes(value);
}
