import assert from "node:assert/strict";

import { createFlatbedClient, Priority } from "./generated/index.js";

const client = createFlatbedClient({ baseUrl: process.env.FLATBED_BASE_URL ?? "http://localhost:8080" });

const echoBody = { message: "hi", times: 3, priority: Priority.High, tags: [{ label: "demo" }] };

const main = (): Promise<void> =>
  client
    .postGreet({ body: { name: "Ada" } })
    .then((res) => {
      console.log("greet  (flatbuffer):", res.greeting);
      assert.equal(res.greeting, "Hello, Ada!");
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
