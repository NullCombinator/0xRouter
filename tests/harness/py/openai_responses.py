"""openai SDK, Responses: one whole and one streamed request."""
import os

from openai import OpenAI

c = OpenAI(base_url=os.environ["ZR_BASE"] + "/v1", api_key=os.environ["ZR_KEY"], max_retries=0)
model = os.environ["ZR_MODEL"]

r = c.responses.create(model=model, input="Say hello.")
assert r.output_text == "Hello", r

text, done = "", None
for e in c.responses.create(model=model, input="Say hello.", stream=True):
    if e.type == "response.output_text.delta":
        text += e.delta
    elif e.type == "response.completed":
        done = e.response
assert text == "Hello", text
assert done is not None and done.output_text == "Hello", done
