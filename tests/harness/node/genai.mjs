// @google/genai SDK, generateContent: one whole and one streamed request.
import assert from "node:assert/strict";
import { ApiError, GoogleGenAI } from "@google/genai";
import { check } from "./failed.mjs";

const ai = new GoogleGenAI({ apiKey: process.env.NR_KEY, httpOptions: { baseUrl: process.env.NR_BASE, apiVersion: "v1beta" } });
const model = process.env.NR_MODEL;

const r = await ai.models.generateContent({ model, contents: "Say hello." });
assert.equal(r.text, "Hello");

let text = "";
for await (const ch of await ai.models.generateContentStream({ model, contents: "Say hello." })) text += ch.text ?? "";
assert.equal(text, "Hello");

await check(ApiError, (m) => ai.models.generateContent({ model: m, contents: "Say hello." }));
await check(ApiError, async (m) => { for await (const _ of await ai.models.generateContentStream({ model: m, contents: "Say hello." })); });
