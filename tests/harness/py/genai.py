"""google-genai SDK, generateContent: one whole and one streamed request."""
import os

from google import genai
from google.genai import types

c = genai.Client(api_key=os.environ["ZR_KEY"], http_options=types.HttpOptions(base_url=os.environ["ZR_BASE"], api_version="v1beta"))
model = os.environ["ZR_MODEL"]

r = c.models.generate_content(model=model, contents="Say hello.")
assert r.text == "Hello", r

text = "".join(ch.text or "" for ch in c.models.generate_content_stream(model=model, contents="Say hello."))
assert text == "Hello", text
