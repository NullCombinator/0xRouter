(module
  
  (memory (export "memory") 1)
  (data (i32.const 32768) "not json \7b")
  (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
  
  (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 140737488355338)
  (@custom "nr.abi" "\01\00\00\00"))
