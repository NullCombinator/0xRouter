// The only file in the kit allowed `unsafe`: the guest side of the host ABI needs raw pointers.
#![allow(unsafe_code)]
//! The guest side of the host ABI (ABI 1): `zr_alloc`, the entry points, the packed result and
//! the `nr.log` import. An author never writes any of it: [`export!`](crate::export) generates
//! the exports for a type implementing [`Adapter`], and `#[unsafe(no_mangle)]` appears only
//! inside that macro.
//!
//! The host writes `{"ctx": …, "parts": […]}` into memory it got from `zr_alloc`, calls the
//! entry point with `(ptr, len)`, and reads the edits back from the packed result
//! `(ptr << 32) | len`. `0` means no edits. Memory is never freed: the host builds a fresh
//! instance for every call.

use serde::Deserialize;

use crate::context::{Context, Direction};
use crate::edit::Edits;
use crate::input::{Adapter, Input, Part};

/// Longest log line, in bytes. The host cuts at the same length.
pub const MAX_LOG_BYTES: usize = 512;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "nr")]
unsafe extern "C" {
    #[link_name = "log"]
    fn host_log(ptr: *const u8, len: usize);
}

/// What the host writes in for a call.
#[derive(Deserialize)]
struct Wire {
    ctx: Context,
    #[serde(default)]
    parts: Vec<Part>,
}

/// Runs `A`'s hook for `direction` over the host's input bytes. `None` is no edits: nothing
/// edited, or an input this kit cannot read (which the host then sees as no change).
pub fn run_bytes<A: Adapter>(direction: Direction, input: &[u8]) -> Option<Vec<u8>> {
    let wire: Wire = serde_json::from_slice(input).ok()?;
    let input = Input { parts: wire.parts };
    let mut out = Edits::default();
    match direction {
        Direction::Request => A::on_request(&wire.ctx, &input, &mut out),
        Direction::Response => A::on_response(&wire.ctx, &input, &mut out),
        Direction::Event => A::on_event(&wire.ctx, &input, &mut out),
    }
    if out.is_empty() {
        return None;
    }
    serde_json::to_vec(&out).ok()
}

/// `(ptr << 32) | len`, the result the host unpacks. `0` is no edits.
pub fn pack(ptr: u32, len: u32) -> i64 {
    ((u64::from(ptr) << 32) | u64::from(len)) as i64
}

/// Reserves `len` bytes for the host to write into, and leaves them reserved.
pub fn alloc(len: u32) -> u32 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as usize as u32
}

/// The entry point body: reads `(ptr, len)`, runs the hook, leaves the answer in memory and
/// returns it packed.
pub fn run<A: Adapter>(direction: Direction, ptr: u32, len: u32) -> i64 {
    // SAFETY: the host wrote `len` bytes at `ptr` into memory `alloc` reserved, and no one
    // else touches it during the call.
    let input = unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len as usize) };
    let Some(out) = run_bytes::<A>(direction, input) else { return 0 };
    let (len, ptr) = (out.len() as u32, out.as_ptr() as usize as u32);
    std::mem::forget(out);
    pack(ptr, len)
}

/// Writes one line to the host's debug log, cut at [`MAX_LOG_BYTES`]. The host redacts it,
/// and drops lines past its per-call budget.
pub fn log(line: &str) {
    let mut end = line.len().min(MAX_LOG_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    #[cfg(target_arch = "wasm32")]
    // SAFETY: the pointer and length name bytes of `line`, which outlives the call.
    unsafe {
        host_log(line.as_ptr(), end);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = &line[..end];
}

/// Generates the exports the host calls for a type implementing [`Adapter`]: `zr_alloc`,
/// `zr_on_request`, `zr_on_response`, `zr_on_event` and the `nr.abi` section.
///
/// ```ignore
/// struct MyHarness;
/// impl nullrouter_adapter_kit::Adapter for MyHarness { /* … */ }
/// nullrouter_adapter_kit::export!(MyHarness);
/// ```
#[macro_export]
macro_rules! export {
    ($adapter:ty) => {
        #[cfg(target_arch = "wasm32")]
        #[used]
        #[unsafe(link_section = "nr.abi")]
        static NR_ABI: [u8; 4] = $crate::KIT_ABI.to_le_bytes();

        #[unsafe(no_mangle)]
        pub extern "C" fn zr_alloc(len: u32) -> u32 {
            $crate::abi::alloc(len)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn zr_on_request(ptr: u32, len: u32) -> i64 {
            $crate::abi::run::<$adapter>($crate::Direction::Request, ptr, len)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn zr_on_response(ptr: u32, len: u32) -> i64 {
            $crate::abi::run::<$adapter>($crate::Direction::Response, ptr, len)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn zr_on_event(ptr: u32, len: u32) -> i64 {
            $crate::abi::run::<$adapter>($crate::Direction::Event, ptr, len)
        }
    };
}

/// Writes a formatted line to the host's debug log, cut at 512 bytes.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::abi::log(&::std::format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Reason;
    use serde_json::json;

    struct Strip;
    impl Adapter for Strip {
        fn on_request(_: &Context, input: &Input, out: &mut Edits) {
            for (path, _) in input.parts() {
                out.remove(path, Reason::TargetRejectsField);
            }
        }
    }

    fn wire(parts: serde_json::Value) -> Vec<u8> {
        json!({"ctx": {"direction": "request", "provider": "p", "target_style": "openai-chat",
                       "same_style": true, "model": "m", "model_type": "text",
                       "capabilities": {}, "stream": false, "attempt": 0},
               "parts": parts})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_hook_that_edits_answers_the_edits_as_json() {
        let input = wire(json!([{"path": "messages[1].reasoning_content", "value": "x"}]));
        let out = run_bytes::<Strip>(Direction::Request, &input).expect("edits");
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["edits"][0]["op"], "remove");
        assert_eq!(v["edits"][0]["path"], "messages[1].reasoning_content");
    }

    #[test]
    fn no_edits_and_unreadable_input_answer_nothing() {
        assert_eq!(run_bytes::<Strip>(Direction::Request, &wire(json!([]))), None);
        assert_eq!(run_bytes::<Strip>(Direction::Request, b"not json"), None);
        // A hook the adapter did not implement edits nothing.
        let input = wire(json!([{"path": "a", "value": 1}]));
        assert_eq!(run_bytes::<Strip>(Direction::Response, &input), None);
    }

    #[test]
    fn the_result_packs_pointer_over_length() {
        assert_eq!(pack(0, 0), 0);
        assert_eq!(pack(3000, 7), (3000_i64 << 32) | 7);
    }

    #[test]
    fn a_log_line_is_cut_at_a_character_boundary() {
        // 511 ASCII bytes then a 3-byte character straddling the 512 cut: must not panic.
        let line = format!("{}€", "a".repeat(511));
        log(&line);
        log("");
    }
}
