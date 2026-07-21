// Words that can't be a method argument name in a generated `async` method
// (ECMAScript reserved, strict-mode reserved, `eval`/`arguments`, `this`).
const TS_KEYWORDS = new Set([
  "arguments", "await", "break", "case", "catch", "class", "const", "continue",
  "debugger", "default", "delete", "do", "else", "enum", "eval", "export",
  "extends", "false", "finally", "for", "function", "if", "implements", "import",
  "in", "instanceof", "interface", "let", "new", "null", "package", "private",
  "protected", "public", "return", "static", "super", "switch", "this", "throw",
  "true", "try", "typeof", "var", "void", "while", "with", "yield",
]);

// Names the generated client can't take: the base class already declares each.
const RESERVED_METHOD_NAMES = new Set(["constructor", "request", "baseUrl", "transport"]);

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

export const isReservedMethodName = (name: string): boolean => RESERVED_METHOD_NAMES.has(name);

/**
 * A path parameter as a valid TS identifier for the generated method — a pure
 * local (argument + URL interpolation), so it can be adjusted: a reserved word
 * gets a trailing `_`, a non-identifier (empty or digit-leading) a leading `_`.
 */
export const paramIdent = (param: string): string => {
  const base = camelCase(param);
  if (TS_KEYWORDS.has(base)) return `${base}_`;
  if (isValidTsIdentifier(base)) return base;
  return `_${base}`;
};

/** The `{param}` names in a path, in order. */
export const pathParams = (path: string): readonly string[] =>
  path
    .split("/")
    .filter((seg) => seg.startsWith("{") && seg.endsWith("}"))
    .map((seg) => seg.slice(1, -1));
