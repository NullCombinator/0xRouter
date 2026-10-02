// openai SDK, Responses: one whole and one streamed request.
import assert from "node:assert/strict";
import OpenAI from "openai";
import { check } from "./failed.mjs";

const c = new OpenAI({ baseURL: `${process.env.NR_BASE}/v1`, apiKey: process.env.NR_KEY, maxRetries: 0 });
const model = process.env.NR_MODEL;

const r = await c.responses.create({ model, input: "Say hello." });
assert.equal(r.output_text, "Hello");

let text = "";
let done = null;
for await (const e of await c.responses.create({ model, input: "Say hello.", stream: true })) {
  if (e.type === "response.output_text.delta") text += e.delta;
  else if (e.type === "response.completed") done = e.response;
}
assert.equal(text, "Hello");
assert.ok(done, "no response.completed event");

await check(OpenAI.APIError, (m) => c.responses.create({ model: m, input: "Say hello." }));
await check(OpenAI.APIError, async (m) => { for await (const _ of await c.responses.create({ model: m, input: "Say hello.", stream: true })); });
