"""google-genai SDK, generateContent: one whole and one streamed request."""
import os

from google import genai
from google.genai import errors, types
from failed import check

c = genai.Client(api_key=os.environ["NR_KEY"], http_options=types.HttpOptions(base_url=os.environ["NR_BASE"], api_version="v1beta"))
model = os.environ["NR_MODEL"]

r = c.models.generate_content(model=model, contents="Say hello.")
assert r.text == "Hello", r

text = "".join(ch.text or "" for ch in c.models.generate_content_stream(model=model, contents="Say hello."))
assert text == "Hello", text

check(errors.APIError, lambda m: c.models.generate_content(model=m, contents="Say hello."))
check(errors.APIError, lambda m: list(c.models.generate_content_stream(model=m, contents="Say hello.")))
