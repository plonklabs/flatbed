import type { FbsSchema, FbsTable, FbsType } from "./model.js";

/** The table names a field's type references directly (a table, or a vector of tables). */
const tableRefs = (t: FbsType): readonly string[] => {
  if (t.kind === "table") return [t.name];
  if (t.kind === "vector" && t.element.kind === "table") return [t.element.name];
  return [];
};

/**
 * The tables reachable from `roots` by following table and vector-of-table
 * fields — the transitive closure a codec must emit to encode or decode any of
 * `roots`. A name in `roots` that isn't a table (an enum, or absent) is ignored.
 */
export const reachableTables = (schema: FbsSchema, roots: Iterable<string>): ReadonlySet<string> => {
  const byName = new Map<string, FbsTable>(schema.tables.map((t) => [t.name, t]));
  const seen = new Set<string>();
  const visit = (name: string): void => {
    const table = byName.get(name);
    if (table === undefined || seen.has(name)) return;
    seen.add(name);
    table.fields.forEach((f) => tableRefs(f.type).forEach(visit));
  };
  [...roots].forEach(visit);
  return seen;
};
