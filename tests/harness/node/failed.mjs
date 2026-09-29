// The all-attempts-failed check (T074, SC-009): with ZR_MODEL_FAIL set (a model whose
// every attempt fails), `call` must throw the SDK's own API error, not a parse error, and
// the message must carry the record id.
import assert from "node:assert/strict";

const fail = process.env.ZR_MODEL_FAIL;

/** Runs `call(model)` against the failing model; a no-op without ZR_MODEL_FAIL. */
export async function check(ErrorType, call) {
  if (!fail) return;
  try {
    await call(fail);
  } catch (e) {
    assert.ok(e instanceof ErrorType, `${fail}: not the SDK's API error: ${e?.constructor?.name}: ${e}`);
    assert.match(String(e.message), /\(record rq_\w+\)/);
    return;
  }
  assert.fail(`${fail}: no error thrown`);
}
