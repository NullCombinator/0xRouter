"""openai SDK, Chat Completions: one whole and one streamed request."""
import os

from openai import APIStatusError, OpenAI
from failed import check

c = OpenAI(base_url=os.environ["ZR_BASE"] + "/v1", api_key=os.environ["ZR_KEY"], max_retries=0)
model = os.environ["ZR_MODEL"]
messages = [{"role": "user", "content": "Say hello."}]

r = c.chat.completions.create(model=model, messages=messages)
assert r.choices[0].message.content == "Hello", r

chunks = c.chat.completions.create(model=model, messages=messages, stream=True, stream_options={"include_usage": True})
text, usage = "", None
for ch in chunks:
    text += "".join(x.delta.content or "" for x in ch.choices)
    usage = ch.usage or usage
assert text == "Hello", text
assert usage is not None and usage.completion_tokens == 2, usage

check(APIStatusError, lambda m: c.chat.completions.create(model=m, messages=messages))
check(APIStatusError, lambda m: list(c.chat.completions.create(model=m, messages=messages, stream=True)))
