(module
  
  (memory (export "memory") 1)
  
  (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
  
  (func (export "zr_on_request") (param i32 i32) (result i64) (drop (memory.grow (i32.const 2048)))
    i64.const 0)
  (@custom "nr.abi" "\01\00\00\00"))
