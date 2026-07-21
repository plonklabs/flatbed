/**
 * camelCase an identifier fragment: `GetUser`, `get_user`, `get-user`, and
 * `GET_USER` all become `getUser`. Non-alphanumeric runs delimit words; a
 * segment with no lowercase (a SCREAMING run) is lowercased wholesale, while a
 * segment with lowercase keeps its own capitals as boundaries.
 */
export const camelCase = (s: string): string =>
  s
    .split(/[^A-Za-z0-9]+/u)
    .filter((seg) => seg.length > 0)
    .map((seg, i) => {
      const rest = /[a-z]/u.test(seg) ? seg.slice(1) : seg.slice(1).toLowerCase();
      const head = i === 0 ? seg[0]!.toLowerCase() : seg[0]!.toUpperCase();
      return head + rest;
    })
    .join("");

/** PascalCase form for a method-name fragment (`user-id` → `UserId`). */
export const pascalCase = (s: string): string => {
  const c = camelCase(s);
  return c.length === 0 ? c : c[0]!.toUpperCase() + c.slice(1);
};

/** A valid TS identifier: starts with a letter/`_`/`$`, then letters/digits/`_`/`$`. */
export const isValidTsIdentifier = (name: string): boolean =>
  /^[A-Za-z_$][A-Za-z0-9_$]*$/u.test(name);

/**
 * A path parameter as a `pathParams` object key. camelCased, with a leading `_`
 * when the result isn't a valid identifier (empty or digit-leading like `2fa`)
 * so it stays dot-accessible. Reserved words are fine — they're valid property
 * names.
 */
export const paramIdent = (param: string): string => {
  const base = camelCase(param);
  return isValidTsIdentifier(base) ? base : `_${base}`;
};

/** The `{param}` names in a path, in order. */
export const pathParams = (path: string): readonly string[] =>
  path
    .split("/")
    .filter((seg) => seg.startsWith("{") && seg.endsWith("}"))
    .map((seg) => seg.slice(1, -1));
