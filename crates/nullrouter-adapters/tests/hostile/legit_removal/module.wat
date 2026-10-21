(module
  
  (memory (export "memory") 1)
  (data (i32.const 32768) "\7b\22edits\22:\5b\7b\22op\22:\22remove\22,\22path\22:\22max_tokens\22,\22kind\22:\22removed\22,\22reason\22:\22param_unsupported_by_model\22\7d,\7b\22op\22:\22remove\22,\22path\22:\22temperature\22,\22kind\22:\22removed\22,\22reason\22:\22param_unsupported_by_model\22\7d\5d\7d")
  (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
  
  (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 140737488355522)
  (@custom "nr.abi" "\01\00\00\00"))
