"""The all-attempts-failed check (T074, SC-009): with ZR_MODEL_FAIL set (a model whose every
attempt fails), `call` must raise the SDK's own API error, not a parse error, and the
message must carry the record id."""
import os
import re

FAIL = os.environ.get("ZR_MODEL_FAIL")


def check(error_type, call):
    """Runs `call(model)` against the failing model; a no-op without ZR_MODEL_FAIL."""
    if not FAIL:
        return
    try:
        call(FAIL)
    except error_type as e:
        m = re.search(r"\(record (rq_\w+)\)", str(e))
        assert m, f"no record id in {e!s}"
        return
    raise AssertionError(f"{FAIL}: no error raised")
