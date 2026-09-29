"""Mid-stream breaks (T090, US4): every SDK's stream parser takes a restarted answer and
an answer ended by the error event. ZR_MODEL_CUT names a model whose first stream for a
body is cut after "w0 w1 w2 "; ZR_KEY_STRICT is a key whose break behaviour is error_event."""
import os

import anthropic
import openai
from google import genai
from google.genai import errors, types

NOTE = "— connection lost, answer restarted —"
base, model = os.environ["ZR_BASE"], os.environ["ZR_MODEL_CUT"]
ask = "Say hello."


def styles(key):
    """(name, api error type, streamed call returning the text), one per style."""
    chat = openai.OpenAI(base_url=base + "/v1", api_key=key, max_retries=0)
    ant = anthropic.Anthropic(base_url=base, api_key=key, max_retries=0)
    gem = genai.Client(api_key=key, http_options=types.HttpOptions(base_url=base, api_version="v1beta"))

    def chat_text(tag):
        s = chat.chat.completions.create(model=model, messages=[{"role": "user", "content": f"{ask} {tag}"}], stream=True)
        return "".join(x.delta.content or "" for ch in s for x in ch.choices)

    def responses_text(tag):
        text = ""
        for e in chat.responses.create(model=model, input=f"{ask} {tag}", stream=True):
            if e.type == "response.output_text.delta":
                text += e.delta
            elif e.type == "error":
                raise openai.APIError(e.message, None, body=None)
        return text

    def messages_text(tag):
        with ant.messages.stream(model=model, max_tokens=64, messages=[{"role": "user", "content": f"{ask} {tag}"}]) as s:
            text = "".join(s.text_stream)
            s.get_final_message()
        return text

    def genai_text(tag):
        return "".join(ch.text or "" for ch in gem.models.generate_content_stream(model=model, contents=f"{ask} {tag}"))

    return [
        ("chat", openai.APIError, chat_text),
        ("responses", openai.APIError, responses_text),
        ("messages", anthropic.APIError, messages_text),
        ("genai", errors.APIError, genai_text),
    ]


# Restart: the cut words, the note, then the new answer, and the parser finishes cleanly.
for name, _, call in styles(os.environ["ZR_KEY"]):
    text = call(f"py-restart-{name}")
    assert text.startswith("w0 w1 w2") and NOTE in text and text.endswith("Hello"), f"{name}: {text!r}"

# Error event: the SDK raises its own API error, or ends the stream; never a parse error.
for name, api_error, call in styles(os.environ["ZR_KEY_STRICT"]):
    try:
        text = call(f"py-error-{name}")
    except api_error:
        continue
    assert "Hello" not in text and NOTE not in text, f"{name}: {text!r}"
