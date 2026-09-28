// openai SDK, Chat Completions: one whole and one streamed request.
import assert from "node:assert/strict";
import OpenAI from "openai";

const c = new OpenAI({ baseURL: `${process.env.ZR_BASE}/v1`, apiKey: process.env.ZR_KEY, maxRetries: 0 });
const model = process.env.ZR_MODEL;
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
