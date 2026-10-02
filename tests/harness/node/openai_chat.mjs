// openai SDK, Chat Completions: one whole and one streamed request.
import assert from "node:assert/strict";
import OpenAI from "openai";
import { check } from "./failed.mjs";

const c = new OpenAI({ baseURL: `${process.env.NR_BASE}/v1`, apiKey: process.env.NR_KEY, maxRetries: 0 });
const model = process.env.NR_MODEL;
const messages = [{ role: "user", content: "Say hello." }];

const r = await c.chat.completions.create({ model, messages });
assert.equal(r.choices[0].message.content, "Hello");

let text = "";
let usage = null;
for await (const ch of await c.chat.completions.create({ model, messages, stream: true, stream_options: { include_usage: true } })) {
  text += ch.choices.map((x) => x.delta.content ?? "").join("");
  usage = ch.usage ?? usage;
}
assert.equal(text, "Hello");
assert.equal(usage?.completion_tokens, 2);

await check(OpenAI.APIError, (m) => c.chat.completions.create({ model: m, messages }));
await check(OpenAI.APIError, async (m) => { for await (const _ of await c.chat.completions.create({ model: m, messages, stream: true })); });
