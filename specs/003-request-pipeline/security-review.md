# Security review: slice 003 (T142)

Run 2026-10-02 with the `security-auditor` agent over the secret paths, SSRF and the
operator socket. Each High finding was reproduced in the code before it was fixed.

## High (fixed)

| # | Finding | Fix | Test |
|---|---|---|---|
| H1 | Schema 1 URLs were checked without the private-host rule, then converted to endpoints after the gate. hyper skips the custom resolver for address-literal hosts, so `base_url = "http://169.254.169.254/…"` reached the network with `allow_private_endpoints = false`. | The gate applies the private-host rule to schema 1 base URLs and capability endpoints (`validate/gate.rs` `schema1_hosts`). At request time, `upstream::check_ip_host` refuses a private address-literal host before sending (attempt and job polls). | `gate/invalid/providers/schema1-url-metadata-ip.toml`, `upstream::tests::address_literal_hosts_are_checked_here` |
| H2 | The gate split the URL authority by hand. `\`, percent-encoded and full-width forms (`169.254.169.254\latest`, `127.0.0.%31`, `１２７.０.０.１`) read as names there but as addresses in the `url` crate reqwest uses. | Hosts are classified with `url::Url::parse`; a backslash, or `%` in the host, is refused. | `ssrf::tests::private_hosts_rejected_unless_allowed`, `gate/invalid/providers/url-backslash-host.toml` |
| H3 | `provider_hosts` left out `token_count` URLs, so a replacing plugin could send counts, with the secret, to a host the account was never bound to. | `token_count` URLs are part of the binding. | `engine/tests/host_binding.rs` |
| H4 | Hand-written accounts were bound to their provider's hosts in memory at every load and never saved, so a replacing plugin re-bound them. | The first binding is saved to `accounts.toml`. | `engine/tests/host_binding.rs` |
| H5 | `[session] header` could name a floor header (`x-api-key`). The client's value is kept, so its agent key went upstream, and the operator's secret could be overwritten. | The gate refuses a session header the floor blocks, and the engine skips one at runtime. | `gate/invalid/providers/session-header-in-floor.toml` |

The community listing verdict is now computed with private endpoints allowed, because it
says whether the core can run a plugin. `plugins install` checks again under the operator's
setting, so the eight self-hosted plugins (ollama-local, comfyui, …) list as fitting and
install only with `allow_private_endpoints = true`.

## Medium and Low (open, not fixed in this slice)

| # | Finding |
|---|---|
| M1 | The client follows `HTTP(S)_PROXY`/`ALL_PROXY`, so with a proxy set, the resolved-address check sees only the proxy. Fix: `.no_proxy()` unless the operator configures one. |
| M2 | `allow_private_endpoints` is global; enabling it for one local server opens it for every plugin. Fix: a per-provider allow list. |
| M3 | `is_private_ip` misses NAT64 (`64:ff9b::/96`), 6to4 (`2002::/16`), IPv4-compatible IPv6, `fec0::/10`, `198.18/15`, `192.0.0/24` and `240/4`. |
| M4 | A key revocation applies only through an all-or-nothing reload; an unrelated broken file keeps a revoked key working until fixed. |
| L1 | The binding compares host names only, not scheme and port. |
| L2 | A TOML parse error in `accounts.toml` quotes the line; a new, malformed secret line isn't known to the redactor yet. |
| L3 | Secrets under 8 characters aren't masked; escaped or encoded echoes aren't matched. |
| L4 | `write_private` uses a predictable temp name without `O_NOFOLLOW` (fails safe: shared files are refused on read). |
| L5 | The operator socket relies on the 0700 `run/` directory and 0600 mode; no peer-UID check. |
| L6 | A secret typed at a terminal is echoed (the CLI warns). |
| L7 | A provider's job id goes into the poll URL unencoded (same host only). |

## Holding

Redirects are off; resolved addresses are checked and the connection uses only them; auth
is inserted last and marked sensitive; static plugin headers pass the floor at runtime;
same-style forwarding applies the floor and drops values carrying a secret or agent key;
fallback never moves a secret across providers; client errors, records and logs are
redacted; keys are stored as SHA-256 digests; secrets enter only by stdin or `--env`; the
socket offers reload, records and account state only.
