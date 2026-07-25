import assert from "node:assert/strict";

import { createFlatbedClient, Priority } from "./generated/index.js";

// The generated client talks to any flatbed service; point it at the running
// examples/openapi server (override with FLATBED_BASE_URL).
const client = createFlatbedClient({ baseUrl: process.env.FLATBED_BASE_URL ?? "http://localhost:8080" });

// A richer request than greet: an enum (`priority`), a `uint32` (`times`), and a
// vector of tables (`tags`) — all handled by the generated codec.
const echoBody = { message: "hi", times: 3, priority: Priority.High, tags: [{ label: "demo" }] };

const main = (): Promise<void> =>
  // FlatBuffer is the default wire format — the call site is a single typed argument.
  client
    .postGreet({ body: { name: "Ada" } })
    .then((res) => {
      console.log("greet  (flatbuffer):", res.greeting);
      assert.equal(res.greeting, "Hello, Ada!");
      // The same operation over JSON, chosen per-call with `{ as: "json" }`.
      return client.postGreet({ body: { name: "Ada" } }, { as: "json" });
    })
    .then((res) => {
      console.log("greet  (json):      ", res.greeting);
      assert.equal(res.greeting, "Hello, Ada!");
      return client.postEcho({ body: echoBody });
    })
    .then((res) => {
      console.log("echo   (flatbuffer):", res.message);
      assert.equal(res.message, "hi hi hi");
      return client.postEcho({ body: echoBody }, { as: "json" });
    })
    .then((res) => {
      console.log("echo   (json):      ", res.message);
      assert.equal(res.message, "hi hi hi");
    });

main().catch((err: unknown) => {
  console.error("example failed:", err);
  process.exit(1);
});
