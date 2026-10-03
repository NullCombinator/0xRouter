"""Token-expiry soak (spec 005 T049): openai SDK traffic in bursts with idle gaps.

Runs for NR_SOAK_SECS (default 45): 3 s of back-to-back whole and streamed requests, then
NR_SOAK_GAP seconds (default 5) idle. Any failed request ends the script with an error.
"""
import os
import time

from openai import OpenAI

c = OpenAI(base_url=os.environ["NR_BASE"] + "/v1", api_key=os.environ["NR_KEY"], max_retries=0)
model = os.environ["NR_MODEL"]
messages = [{"role": "user", "content": "Say hello."}]
end = time.monotonic() + float(os.environ.get("NR_SOAK_SECS", "45"))
gap = float(os.environ.get("NR_SOAK_GAP", "5"))
n = 0
while time.monotonic() < end:
    burst = time.monotonic() + 3
    while time.monotonic() < burst:
        r = c.chat.completions.create(model=model, messages=messages)
        assert r.choices[0].message.content == "Hello", r
        chunks = c.chat.completions.create(model=model, messages=messages, stream=True)
        text = "".join(x.delta.content or "" for ch in chunks for x in ch.choices)
        assert text == "Hello", text
        n += 2
    time.sleep(gap)
print(f"python soak: {n} requests, none failed")
