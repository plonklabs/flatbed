import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import type { FbsSchema, FbsTable } from "./model.js";
import { readBfbs } from "./read-bfbs.js";

const fixture = (): FbsSchema =>
  readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

const table = (schema: FbsSchema, name: string): FbsTable => {
  const found = schema.tables.find((t) => t.name === name);
  assert.ok(found, `table ${name} present`);
  return found;
};

const field = (t: FbsTable, name: string) => {
  const found = t.fields.find((f) => f.name === name);
  assert.ok(found, `field ${name} present on ${t.name}`);
  return found;
};

test("reads the enum's underlying type and values in order", () => {
  const severity = fixture().enums.find((e) => e.name === "Severity");
  assert.ok(severity);
  assert.equal(severity.underlying, "int8");
  assert.deepEqual(
    severity.members.map((m) => [m.name, Number(m.value)]),
    [
      ["Info", 0],
      ["Warning", 1],
      ["Error", 2],
    ],
  );
});

test("reads scalar, string, and bool fields with their wire widths", () => {
  const req = table(fixture(), "TestResponse");
  assert.deepEqual(field(req, "value").type, { kind: "scalar", scalar: "uint64" });
  assert.deepEqual(field(req, "message").type, { kind: "string" });
  assert.deepEqual(field(req, "success").type, { kind: "scalar", scalar: "bool" });
});

test("reads a nested table field as a table reference", () => {
  const user = table(fixture(), "UserRequest");
  assert.deepEqual(field(user, "address").type, { kind: "table", name: "Address" });
});

test("reads vectors of tables, strings, and enums", () => {
  const book = table(fixture(), "AddressBook");
  assert.deepEqual(field(book, "addresses").type, {
    kind: "vector",
    element: { kind: "table", name: "Address" },
  });
  assert.deepEqual(field(book, "contact_names").type, {
    kind: "vector",
    element: { kind: "string" },
  });
  const log = table(fixture(), "LogEvent");
  assert.deepEqual(field(log, "severity").type, { kind: "enum", name: "Severity" });
  assert.deepEqual(field(log, "history").type, {
    kind: "vector",
    element: { kind: "enum", name: "Severity" },
  });
});

test("reads declared defaults for scalars, bools, and enums", () => {
  const d = table(fixture(), "Defaulted");
  assert.deepEqual(field(d, "count").default, { kind: "int", value: 25n });
  assert.deepEqual(field(d, "flag").default, { kind: "int", value: 1n });
  assert.deepEqual(field(d, "ratio").default, { kind: "real", value: 1.5 });
  // `level: Severity = Warning` → the enum's Warning value (1).
  assert.deepEqual(field(d, "level").default, { kind: "int", value: 1n });
});

test("fields are ordered by wire id", () => {
  const d = table(fixture(), "Defaulted");
  assert.deepEqual(
    d.fields.map((f) => f.id),
    [...d.fields.map((f) => f.id)].sort((a, b) => a - b),
  );
});
