import assert from "node:assert/strict";
import { test } from "node:test";

import { readOperations } from "./read-openapi.js";

const fb = { schema: { type: "string" } };
const json = (ref: string) => ({ schema: { $ref: `#/components/schemas/${ref}` } });

const spec = {
  paths: {
    "/echo": {
      post: {
        operationId: "echo",
        requestBody: { content: { "application/json": json("TestRequest"), "application/x-flatbuffers": fb } },
        responses: {
          "200": { content: { "application/json": json("TestResponse"), "application/x-flatbuffers": fb } },
        },
      },
    },
    // JSON-only — not advertising x-flatbuffers, so skipped.
    "/health": { get: { responses: { "200": { content: { "application/json": json("Health") } } } } },
    // Success under a non-200 code, response-only (no request body).
    "/users/{id}": {
      get: {
        responses: {
          "201": { content: { "application/x-flatbuffers": fb, "application/json": json("UserResponse") } },
        },
      },
    },
  },
};

test("picks only operations advertising application/x-flatbuffers", () => {
  assert.deepEqual(
    readOperations(spec).map((o) => `${o.method} ${o.path}`),
    ["POST /echo", "GET /users/{id}"],
  );
});

test("parses operationId and $ref request/response types", () => {
  const echo = readOperations(spec).find((o) => o.path === "/echo");
  assert.equal(echo?.operationId, "echo");
  assert.equal(echo?.requestType, "TestRequest");
  assert.equal(echo?.responseType, "TestResponse");
});

test("reads a success response under a non-200 code, request-less", () => {
  const users = readOperations(spec).find((o) => o.path === "/users/{id}");
  assert.equal(users?.responseType, "UserResponse");
  assert.equal(users?.requestType, undefined);
});

test("picks the lowest 2xx code when a route has multiple", () => {
  const got = readOperations({
    paths: {
      "/upload": {
        post: {
          requestBody: { content: { "application/x-flatbuffers": fb } },
          responses: {
            "201": { content: { "application/x-flatbuffers": fb, "application/json": json("Upload201") } },
            "200": { content: { "application/x-flatbuffers": fb, "application/json": json("Upload200") } },
          },
        },
      },
    },
  });
  assert.equal(got[0]?.responseType, "Upload200");
});

test("returns nothing for a spec with no paths", () => {
  assert.deepEqual(readOperations({}), []);
  assert.deepEqual(readOperations(null), []);
});
