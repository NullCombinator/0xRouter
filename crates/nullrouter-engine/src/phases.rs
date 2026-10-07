//! The one phase definition (spec 013, research R1, R14, R17).
//!
//! `phases::of` turns a request record into per-attempt phases. The records view, the list
//! column, the live view and the summaries all call it. It is pure.
