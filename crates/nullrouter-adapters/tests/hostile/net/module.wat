(module
  (import "wasi_snapshot_preview1" "sock_open" (func (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  
  (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
  
  (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 0)
  (@custom "nr.abi" "\01\00\00\00"))
