"""openai SDK, Chat Completions: one multi-turn session through a unified model (spec 006, US1).
Every turn resends the whole conversation, so its prefix is the previous turn's. The server
checks afterwards that all of these requests were served by one account."""
import os

from openai import OpenAI

c = OpenAI(base_url=os.environ["NR_BASE"] + "/v1", api_key=os.environ["NR_KEY"], max_retries=0)
model = os.environ["NR_MODEL_WARM"]
messages = [
    {"role": "system", "content": "You are a careful assistant. Answer in one short sentence and never guess."},
]
for turn in range(4):
    messages.append({"role": "user", "content": f"Question number {turn}: say something short."})
    r = c.chat.completions.create(model=model, messages=messages)
    text = r.choices[0].message.content
    assert text, r
    messages.append({"role": "assistant", "content": text})
