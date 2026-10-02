// anthropic SDK, Messages: one whole and one streamed request.
import assert from "node:assert/strict";
import Anthropic from "@anthropic-ai/sdk";
import { check } from "./failed.mjs";

const c = new Anthropic({ baseURL: process.env.NR_BASE, apiKey: process.env.NR_KEY, maxRetries: 0 });
const model = process.env.NR_MODEL;
const messages = [{ role: "user", content: "Say hello." }];

const r = await c.messages.create({ model, max_tokens: 64, messages });
assert.equal(r.content[0].text, "Hello");

const s = c.messages.stream({ model, max_tokens: 64, messages });
let text = "";
s.on("text", (t) => (text += t));
const final = await s.finalMessage();
assert.equal(text, "Hello");
assert.equal(final.stop_reason, "end_turn");
assert.equal(final.usage.output_tokens, 2);

await check(Anthropic.APIError, (m) => c.messages.create({ model: m, max_tokens: 64, messages }));
await check(Anthropic.APIError, (m) => c.messages.stream({ model: m, max_tokens: 64, messages }).finalMessage());
