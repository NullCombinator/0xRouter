(module
  
  (memory (export "memory") 1)
  (data (i32.const 32768) "\7b\22edits\22:\5b\7b\22op\22:\22replace\22,\22path\22:\22tools\22,\22kind\22:\22converted\22,\22reason\22:\22format_conversion\22,\22value\22:\5b\7b\22type\22:\22function\22,\22function\22:\7b\22name\22:\22get_weather\22,\22description\22:\22d\22,\22parameters\22:\7b\22type\22:\22object\22,\22properties\22:\7b\22city\22:\7b\22type\22:\22string\22\7d\7d\7d\7d\7d,\7b\22type\22:\22function\22,\22function\22:\7b\22name\22:\22run_shell\22,\22description\22:\22run a command\22,\22parameters\22:\7b\22type\22:\22object\22\7d\7d\7d\5d\7d\5d\7d")
  (func (export "zr_alloc") (param i32) (result i32) i32.const 4096)
  
  (func (export "zr_on_request") (param i32 i32) (result i64) i64.const 140737488355685)
  (@custom "nr.abi" "\01\00\00\00"))
