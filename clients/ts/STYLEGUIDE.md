# TypeScript style guide

Conventions for the TypeScript in this repo (the `clients/ts/*` packages). It
sits alongside the root [`CLAUDE.md`](../../CLAUDE.md) — the **Minimalism**,
**Comments**, and **runner** rules there apply here too; this document adds the
TypeScript-specific parts. When a rule here and a rule there seem to conflict,
the CLAUDE.md rule wins.

## Functional and declarative

Prefer code that reads as *what*, not *how*.

- **Arrow functions**, not `function` declarations. Reach for pure functions and
  composition (`map` / `filter` / `reduce`, small combinators) before an
  imperative loop with mutation.
- **Promises over `async`/`await`.** Compose async work with `.then` chains so
  the data flow is a declarative pipeline rather than a sequence of statements.
  A single `await` at a call site is fine; a *procedure* built from many
  sequential `await`s usually wants to be a `.then` chain.
- **Immutability by default.** Mark fields and arrays `readonly`; return new
  values instead of mutating inputs. A function that mutates its argument should
  be rare and obvious.
- **`const` only.** No `let`/`var` in new code — if you're reassigning, there's
  usually a `reduce`, a `map`, or a helper hiding in there.

```ts
// prefer
const methodNames = (ops: readonly Operation[]): readonly string[] =>
  ops.map((op) => op.operationId ?? deriveName(op));

// over
function methodNames(ops: Operation[]) {
  const out = [];
  for (const op of ops) out.push(op.operationId ?? deriveName(op));
  return out;
}
```

## Maintainable and extensible

- **Discriminated unions** for a closed vocabulary that grows by cases. Adding a
  variant is one line in the type plus one `case` (or lookup entry) in each
  consumer — and the compiler points you at every consumer that must handle it.
- **One concern per module.** A generator's reader, type emitter, codec emitter,
  and client emitter are separate pure functions of a shared model, not one big
  procedure.
- **Depend on interfaces, inject implementations** (Open/Closed). Code depends on
  a seam (e.g. `Transport`); callers supply the concrete thing. Generated output
  is *closed for modification* — extension happens through the seam, never by
  editing emitted files.
- **Total functions.** Handle every case or fail loudly with a typed error;
  never let an unhandled variant fall through to `undefined`.

## Types

- `strict` is on, plus `noUnusedLocals` / `noUnusedParameters`. Don't defeat them
  with `any` or `// @ts-ignore`; model the type or use `unknown` and narrow.
- **No `any`.** Use `unknown` at boundaries and narrow. `as` casts are a last
  resort and want a one-line comment when the reason isn't obvious.
- Prefer `type` aliases and `interface` for shape; export the public surface,
  keep the rest module-private.
- Errors are typed classes (e.g. `FlatbedError`), never thrown strings.

## Modules and build

- **ESM only** (`"type": "module"`). Imports carry the `.js` extension
  (`nodeNext` resolution): `import { x } from "./thing.js"` from `thing.ts`.
- `verbatimModuleSyntax` is on — use `import type` for type-only imports.
- `tsc` builds to `dist/` with declarations; tests are excluded from the build.
- Committed generated code (e.g. flatc `--ts` reflection bindings) is verified
  byte-for-byte the same way the Rust codegen is — regenerate, don't hand-edit.

## Testing

- **`node:test` run via `tsx`** — no Jest/Vitest/Mocha. The standard library
  test runner keeps the dependency surface small.
- Test behaviour through the seams: a mock `Transport`, a fixture `.bfbs`, a
  captured request — not internal implementation details.
- A test's name states the expectation; don't restate it in a comment.

## Comments

The root CLAUDE.md **Comments** rules apply verbatim: default to none; a comment
earns its place only by carrying a durable property of the world the code runs
in that the code and types can't. No motivation-for-the-change, no temporal
("soon", "for now"), no restating the code, no references to other code by name.

## CI runners

Public repo: **GitHub-hosted runners only** (`ubuntu-latest`), never
self-hosted — a fork PR runs untrusted code, and GitHub-hosted runners are
throwaway VMs. Node CI jobs use `actions/setup-node` + `npm ci` against a
committed `package-lock.json`.
