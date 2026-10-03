// Token-expiry soak (spec 005 T049): openai SDK traffic in bursts with idle gaps.
// Runs for NR_SOAK_SECS (default 45): 3 s of back-to-back whole and streamed requests,
// then NR_SOAK_GAP seconds (default 5) idle. Any failed request ends it with an error.
import assert from "node:assert/strict";
import OpenAI from "openai";

const c = new OpenAI({ baseURL: `${process.env.NR_BASE}/v1`, apiKey: process.env.NR_KEY, maxRetries: 0 });
const model = process.env.NR_MODEL;
const messages = [{ role: "user", content: "Say hello." }];
const end = Date.now() + 1000 * Number(process.env.NR_SOAK_SECS ?? 45);
const gap = 1000 * Number(process.env.NR_SOAK_GAP ?? 5);
let n = 0;
while (Date.now() < end) {
  const burst = Date.now() + 3000;
  while (Date.now() < burst) {
    const r = await c.chat.completions.create({ model, messages });
    assert.equal(r.choices[0].message.content, "Hello");
    let text = "";
    for await (const ch of await c.chat.completions.create({ model, messages, stream: true })) {
      text += ch.choices.map((x) => x.delta.content ?? "").join("");
    }
    assert.equal(text, "Hello");
    n += 2;
  }
  await new Promise((r) => setTimeout(r, gap));
}
console.log(`node soak: ${n} requests, none failed`);
