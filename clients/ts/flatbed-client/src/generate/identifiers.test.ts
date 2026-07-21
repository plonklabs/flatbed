import assert from "node:assert/strict";
import { test } from "node:test";

import { camelCase, isValidTsIdentifier, pascalCase } from "./identifiers.js";

test("camelCase normalizes every casing convention to the same result", () => {
  assert.equal(camelCase("GetUser"), "getUser");
  assert.equal(camelCase("get_user"), "getUser");
  assert.equal(camelCase("get-user"), "getUser");
  assert.equal(camelCase("GET_USER"), "getUser"); // screaming snake → lowercased wholesale
  assert.equal(camelCase("getUser"), "getUser");
});

test("camelCase lowercases a single screaming word", () => {
  assert.equal(camelCase("GET"), "get");
});

test("pascalCase upper-cases the leading character", () => {
  assert.equal(pascalCase("user-id"), "UserId");
  assert.equal(pascalCase("GET_USER"), "GetUser");
});

test("isValidTsIdentifier accepts leading letter/_/$ and rejects a leading digit", () => {
  assert.ok(isValidTsIdentifier("getUser"));
  assert.ok(isValidTsIdentifier("_x"));
  assert.ok(!isValidTsIdentifier("3d"));
  assert.ok(!isValidTsIdentifier(""));
});
