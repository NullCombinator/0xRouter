# Contract: Decision record addition (FR-013)

Each `candidates[]` row of a decision record (slice 006, `contracts/record-journal.md`) gains an
optional field:

```json
"meter_sources": {
  "weekly": {"capacity": "fit", "weight.output": "plugin_override"},
  "5-hour": {"multiplier.claude-opus-*": "account_override"}
}
```

- Only windows the candidate's quota state read, and only numbers whose source isn't
  `declared`, are listed.
- The field is **absent** when every number in effect is declared. Records written before a
  fit is significant or an override is set are byte-identical to slice 006's (research R13).
- Values: `account_override`, `plugin_override`, `fit`.
- Readers (slice 008's read model, `records` CLI) ignore unknown fields today. The read model
  shows the map as is in record detail. No query filters on it in this slice.
