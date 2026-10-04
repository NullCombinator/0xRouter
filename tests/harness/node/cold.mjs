// openai SDK, Chat Completions: whole and streamed cold requests through a unified model
// (spec 006, US3). The server checks afterwards where they were served; the text is not asserted
// because the provider behind the model is the prompt-cache simulation.
import assert from "node:assert/strict";
import OpenAI from "openai";

const c = new OpenAI({ baseURL: `${process.env.NR_BASE}/v1`, apiKey: process.env.NR_KEY, maxRetries: 0 });
const model = process.env.NR_MODEL_COLD;
const messages = (n) => [
  { role: "system", content: `You are node assistant number ${n}. Answer in one short sentence.` },
  { role: "user", content: "Say something short." },
];

const r = await c.chat.completions.create({ model, messages: messages(1) });
assert.ok(r.choices[0].message.content);

let text = "";
for await (const ch of await c.chat.completions.create({ model, messages: messages(2), stream: true })) {
  text += ch.choices.map((x) => x.delta.content ?? "").join("");
}
assert.ok(text);
