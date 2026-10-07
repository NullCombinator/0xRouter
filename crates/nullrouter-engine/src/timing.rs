//! Per-attempt marks (spec 013, research R1–R3).
//!
//! An `AttemptClock` holds the marks of the running attempt; it is folded into the record's
//! `AttemptTiming` when the attempt ends.
