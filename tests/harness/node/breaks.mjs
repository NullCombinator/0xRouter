// Mid-stream breaks (T090, US4): every SDK's stream parser takes a restarted answer and
// an answer ended by the error event. ZR_MODEL_CUT names a model whose first stream for a
// body is cut after "w0 w1 w2 "; ZR_KEY_STRICT is a key whose break behaviour is error_event.
import assert from "node:assert/strict";
import Anthropic from "@anthropic-ai/sdk";
import { ApiError, GoogleGenAI } from "@google/genai";
import OpenAI from "openai";

const NOTE = "— connection lost, answer restarted —";
const base = process.env.ZR_BASE;
const model = process.env.ZR_MODEL_CUT;
const ask = "Say hello.";

/** [name, api error type, streamed call returning the text], one per style. */
function styles(apiKey) {
  const chat = new OpenAI({ baseURL: `${base}/v1`, apiKey, maxRetries: 0 });
  const ant = new Anthropic({ baseURL: base, apiKey, maxRetries: 0 });
  const gem = new GoogleGenAI({ apiKey, httpOptions: { baseUrl: base, apiVersion: "v1beta" } });
  return [
    ["chat", OpenAI.APIError, async (tag) => {
      let text = "";
      for await (const ch of await chat.chat.completions.create({ model, messages: [{ role: "user", content: `${ask} ${tag}` }], stream: true })) {
        text += ch.choices.map((x) => x.delta.content ?? "").join("");
      }
      return text;
    }],
    ["responses", OpenAI.APIError, async (tag) => {
      let text = "";
      for await (const e of await chat.responses.create({ model, input: `${ask} ${tag}`, stream: true })) {
        if (e.type === "response.output_text.delta") text += e.delta;
        else if (e.type === "error") throw new OpenAI.APIError(undefined, e, e.message, undefined);
      }
      return text;
    }],
    ["messages", Anthropic.APIError, async (tag) => {
      const s = ant.messages.stream({ model, max_tokens: 64, messages: [{ role: "user", content: `${ask} ${tag}` }] });
      let text = "";
      s.on("text", (t) => (text += t));
      await s.finalMessage();
      return text;
    }],
    ["genai", ApiError, async (tag) => {
      let text = "";
      for await (const ch of await gem.models.generateContentStream({ model, contents: `${ask} ${tag}` })) text += ch.text ?? "";
      return text;
    }],
  ];
}

// Restart: the cut words, the note, then the new answer, and the parser finishes cleanly.
for (const [name, , call] of styles(process.env.ZR_KEY)) {
  const text = await call(`node-restart-${name}`);
  assert.ok(text.startsWith("w0 w1 w2") && text.includes(NOTE) && text.endsWith("Hello"), `${name}: ${JSON.stringify(text)}`);
}

// Error event: the SDK throws its own API error, or ends the stream; never a parse error.
for (const [name, ApiErrorType, call] of styles(process.env.ZR_KEY_STRICT)) {
  let text;
  try {
    text = await call(`node-error-${name}`);
  } catch (e) {
    assert.ok(e instanceof ApiErrorType, `${name}: not the SDK's API error: ${e?.constructor?.name}: ${e}`);
    continue;
  }
  assert.ok(!text.includes("Hello") && !text.includes(NOTE), `${name}: ${JSON.stringify(text)}`);
}
