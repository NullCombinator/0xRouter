"""anthropic SDK, Messages: one whole and one streamed request."""
import os

from anthropic import Anthropic

c = Anthropic(base_url=os.environ["ZR_BASE"], api_key=os.environ["ZR_KEY"], max_retries=0)
model = os.environ["ZR_MODEL"]
messages = [{"role": "user", "content": "Say hello."}]

r = c.messages.create(model=model, max_tokens=64, messages=messages)
assert r.content[0].text == "Hello", r

with c.messages.stream(model=model, max_tokens=64, messages=messages) as s:
    text = "".join(s.text_stream)
    final = s.get_final_message()
assert text == "Hello", text
assert final.stop_reason == "end_turn" and final.usage.output_tokens == 2, final
