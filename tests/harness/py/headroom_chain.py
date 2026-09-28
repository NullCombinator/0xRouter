"""Client → headroom → 0router (SC-014, US1-11): the anthropic and openai SDKs through a
headroom proxy, each to a same-style and a cross-style model, whole and streamed.

Each request carries an unknown body field and header, the way an optimizer upstream of
0router adds its own; the Rust side checks where they arrived and what was recorded.
"""
import os

from anthropic import Anthropic
from openai import OpenAI

base, key = os.environ["HR_BASE"], os.environ["ZR_KEY"]
chat_model, messages_model = os.environ["ZR_MODEL"], os.environ["ZR_MODEL_MESSAGES"]


def marks(who):
    return {"extra_body": {"x_chain": who}, "extra_headers": {"x-chain-marker": who}}


a = Anthropic(base_url=base, api_key=key, max_retries=0)
msgs = [{"role": "user", "content": "Say hello."}]
for model in (messages_model, chat_model):
    r = a.messages.create(model=model, max_tokens=64, messages=msgs, **marks("anthropic"))
    assert r.content[0].text == "Hello", (model, r)
    with a.messages.stream(model=model, max_tokens=64, messages=msgs, **marks("anthropic")) as s:
        text = "".join(s.text_stream)
    assert text == "Hello", (model, text)

o = OpenAI(base_url=base + "/v1", api_key=key, max_retries=0)
for model in (chat_model, messages_model):
    r = o.chat.completions.create(model=model, messages=msgs, **marks("openai"))
    assert r.choices[0].message.content == "Hello", (model, r)
    chunks = o.chat.completions.create(model=model, messages=msgs, stream=True, **marks("openai"))
    text = "".join(x.delta.content or "" for ch in chunks for x in ch.choices)
    assert text == "Hello", (model, text)
