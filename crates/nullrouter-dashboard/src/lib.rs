//! The read-only web dashboard served by `nullrouter serve` on its own loopback port.
//!
//! Pages are server-rendered from the same read model the CLI prints (`nullrouter_server::views`);
//! nothing here changes state. Routes, headers and access rules:
//! `specs/009-dashboard/contracts/dashboard-http.md`.
