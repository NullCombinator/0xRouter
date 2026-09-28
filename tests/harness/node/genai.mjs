// @google/genai SDK, generateContent: one whole and one streamed request.
import assert from "node:assert/strict";
import { GoogleGenAI } from "@google/genai";

const ai = new GoogleGenAI({ apiKey: process.env.ZR_KEY, httpOptions: { baseUrl: process.env.ZR_BASE, apiVersion: "v1beta" } });
const model = process.env.ZR_MODEL;

const r = await ai.models.generateContent({ model, contents: "Say hello." });
assert.equal(r.text, "Hello");

let text = "";
for await (const ch of await ai.models.generateContentStream({ model, contents: "Say hello." })) text += ch.text ?? "";
assert.equal(text, "Hello");
