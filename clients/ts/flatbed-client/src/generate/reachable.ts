import type { FbsSchema, FbsTable, FbsType, FbsUnion } from "./model.js";

/**
 * The table names a field's type references directly: a table, a vector of
 * tables, or every member of a union.
 */
const tableRefs =
  (unions: ReadonlyMap<string, FbsUnion>) =>
  (t: FbsType): readonly string[] => {
    if (t.kind === "table") return [t.name];
    if (t.kind === "vector" && t.element.kind === "table") return [t.element.name];
    if (t.kind === "union") return (unions.get(t.name)?.members ?? []).map((m) => m.table);
    return [];
  };

/**
 * The tables reachable from `roots` by following table and vector-of-table
 * fields — the transitive closure a codec must emit to encode or decode any of
 * `roots`. A name in `roots` that isn't a table (an enum, or absent) is ignored.
 */
export const reachableTables = (schema: FbsSchema, roots: Iterable<string>): ReadonlySet<string> => {
  const byName = new Map<string, FbsTable>(schema.tables.map((t) => [t.name, t]));
  const refs = tableRefs(new Map((schema.unions ?? []).map((u) => [u.name, u])));
  const seen = new Set<string>();
  const visit = (name: string): void => {
    const table = byName.get(name);
    if (table === undefined || seen.has(name)) return;
    seen.add(name);
    table.fields.forEach((f) => refs(f.type).forEach(visit));
  };
  [...roots].forEach(visit);
  return seen;
};
