"""openai SDK, Chat Completions: cold one-shots, then an overflow burst, through a unified model
(spec 006, US3). Every request carries its own system prompt, so none has a warm prefix. The
server checks afterwards where each one was served."""
import os
from concurrent.futures import ThreadPoolExecutor

from openai import OpenAI

c = OpenAI(base_url=os.environ["NR_BASE"] + "/v1", api_key=os.environ["NR_KEY"], max_retries=0)
model = os.environ["NR_MODEL_COLD"]


def one(n):
    r = c.chat.completions.create(
        model=model,
        messages=[
            {"role": "system", "content": f"You are assistant number {n}. Answer in one short sentence."},
            {"role": "user", "content": f"Question {n}: say something short."},
        ],
    )
    assert r.choices[0].message.content, r


for n in range(4):
    one(n)
# More than the subscriptions take: the rest has to overflow, and none may fail.
with ThreadPoolExecutor(max_workers=6) as pool:
    list(pool.map(one, range(100, 112)))
