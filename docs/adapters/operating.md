# Operating harness adapters

A harness adapter changes requests for one client harness before they reach a provider. This page
covers installing one, reviewing and approving it, clearing a suspect version, reading alerts,
using the catalogue, and upgrading after a kit change. To write an adapter, see
[authoring.md](authoring.md).

Adapters are third-party code. Read the safety model before you install one.

## The safety model

An adapter is WASM code run by the core. It has these limits:

- It has no network, no file access, no clock and no secrets. It sees only the matched parts of
  one request or response, and its context has no key, header or session id.
- It returns a list of edits. The core applies them, and no other output is taken.
- Each call has a time limit and a memory limit. A failed call leaves the original body in place
  for that attempt.

Three checks run before an adapter serves:

1. **The gate** reads the package source at install. It refuses unsafe code, foreign
   dependencies, build scripts, and blobs that could hide code. See
   [authoring.md](authoring.md#what-the-gate-refuses).
2. **The build** compiles the source with a pinned toolchain. The builder compiles it twice and
   refuses the module if the two outputs differ.
3. **The review and the approval** are yours. A reviewer model reads the source and writes a
   report. Nothing serves until you approve that version.

At run time, the **guardrail** checks every edit. It refuses an adapter that adds or changes a
tool call, a tool definition, a tool result, an opaque block, or a field the core can't place.
Removing content is allowed. When the guardrail catches a violation, the core:

- drops that adapter's edits and sends the unmodified request;
- raises a `guardrail` alert with the run that caused it;
- marks the version `suspect`. A suspect version stays the active one but serves nothing until
  you clear it.

Secrets are never given to an adapter. The core sends the request and injects secrets only when
it calls the provider.

## Configuration

These settings are in the `[adapters]` table of the operator config. See
[operator-config.md](../operator-config.md).

| Key | Default | Range |
|---|---|---|
| `catalogue_url` | a built-in `https://` index URL | must be `https://` |
| `builder` | absent: `nullrouter-builder` on `PATH` | path |
| `request_deadline_ms` | 20 | 1–1000 |
| `event_deadline_ms` | 2 | 1–100 |
| `memory_mib` | 64 | 1–512 |
| `max_instances` | 64 | 1–4096 |

`request_deadline_ms` bounds each request or response call, and `event_deadline_ms` each stream
event call. `memory_mib` caps an adapter's linear memory, and `max_instances` sizes the pool of
sandbox instances that calls draw from. The four limits are read when `serve` starts; restart it
to apply a change.

If `builder` is absent and `nullrouter-builder` is not on `PATH`, an install stops at `queued`.
It builds when the builder is available. Requeue it with `adapters build`.

## Commands

Add `--json` where a command supports it, for machine output.

### List and show

```bash
nullrouter adapters list
nullrouter adapters show <harness> [<version-id>]
```

`list` shows every harness. The built-in `hermes` shows as `built-in`. `show` prints a version's
manifest, state, review report and decision. It defaults to the active version. If no version is
active, it shows the newest one.

### Install

```bash
nullrouter adapters install <directory-or-tar.gz>
```

The install runs the gate, stores the package, and queues the build. A refusal prints each
`refused:` line and exits 3. When the build finishes, the version goes to `in_review` and the
review starts. If the server is not running, the review starts when `serve` starts.

### Review

```bash
nullrouter adapters review <harness> <version-id>
nullrouter adapters review <harness> <version-id> --retry
```

The first form prints the report: risk, summary and findings. `--retry` requeues a review that
was quarantined because the reviewer returned no valid report.

The reviewer model is set in `review-settings`. Until one is set, a version waits with the reason
`no_review_model: no review model set`.

```bash
nullrouter adapters review-settings --model <model> --budget <tokens> [--reserve-output <tokens>]
nullrouter adapters review-settings --clear
```

`--model` is a unified model id or `provider/model`. `--budget` is the most input tokens one review
may send. `--reserve-output` keeps tokens free for the answer. `--model` and `--budget` are
required together, and `--model` can't be combined with `--clear`.

The reviewer sends the adapter's source to the model you set. Pick a model you trust with it.

### Approve, reject and build

```bash
nullrouter adapters approve <harness> <version-id> [--note "<text>"]
nullrouter adapters reject <harness> <version-id> [--note "<text>"]
nullrouter adapters build <harness> <version-id> [--retry]
```

`approve` makes the version the active one and supersedes the one it replaces. `reject` ends the
version. `build --retry` requeues a build that waited for the builder.

### Clear a suspect version

```bash
nullrouter adapters clear <harness> <version-id>
nullrouter adapters clear <harness> <version-id> --yes
```

`clear` lists the guardrail events for the version first, then asks for confirmation. It makes
the version serve again from the next request. Without `--yes` it reads the answer from the
terminal, and `--json` needs `--yes`.

Before you clear, check the events. A guardrail event means the adapter tried to change content
it may not change.

### Remove

```bash
nullrouter adapters remove <harness> [<version-id>] [--force] [--yes]
```

`remove` lists the keys bound to the harness first, then asks unless `--yes`. Those keys become
plain clients with no adapter. Without a version, it removes the whole harness. `--force` allows
removing the active version.

### Rebuild after a kit change

```bash
nullrouter adapters rebuild [<harness>]
```

Each built module records the kit ABI it was built for. When the core no longer runs that ABI,
`serve` rebuilds the approved version at startup from its reviewed source. The rebuild keeps the same source fingerprint, so it
needs no new review. This command does the same work in the foreground. Without a harness name,
it rebuilds every version that needs it.

A version that fails to rebuild gets the `rebuild_failed` flag and an alert. The command exits 1
if any rebuild failed.

## Version states

| State | Meaning |
|---|---|
| `queued` | Waiting for the builder, or for a build to start |
| `building` | The builder is compiling the source |
| `in_review` | The reviewer is reading the source |
| `quarantined` | The review produced no valid report. Requeue it with `review --retry` |
| `reported` | The report is ready. Waiting for your decision |
| `approved` | Serving, if it is the active version |
| `suspect` | The guardrail caught a violation. Serves nothing until cleared |
| `refused` | The gate or the build refused the package |
| `rejected` | You rejected it |
| `superseded` | A newer version was approved in its place |

## Alerts

```bash
nullrouter alerts list
nullrouter alerts list --all
nullrouter alerts ack <alert-id>
nullrouter alerts ack --all
```

`list` shows open alerts, newest first. `--all` includes acknowledged ones. `ack` takes an alert
id, such as `al_` followed by 10 lowercase letters or digits. `ack --all` acknowledges every open
alert. The log holds at most 500 alerts. Past that, the oldest acknowledged ones go first, then
the oldest.

| Kind | Raised when |
|---|---|
| `guardrail` | The guardrail dropped an adapter's edits. The version becomes `suspect` |
| `adapter_failed` | An adapter call failed. Repeats within 60 s fold into one alert with a count |
| `source_mismatch` | A stored source or module failed its fingerprint check |
| `module_refused` | The sandbox refused to load a module |
| `rebuild_failed` | A rebuild after a kit change failed |
| `quarantined` | A review produced no valid report |
| `refused` | The gate or the build refused a package |

Alerts are read from `adapters/alerts.toml` in the home directory. The `alerts` commands read the
file directly and work while the server is stopped.

To see the adapter run for one request, use `nullrouter records show`. It prints each attempt's
adapter run, its changes, any guardrail result with the id of the alert it raised, and the
response adapter. The agent line names the key's harness and version.

## Catalogue

The catalogue lists adapters you can install without a local package.

```bash
nullrouter catalogue list
nullrouter catalogue show <harness>
nullrouter catalogue install <harness> [<semver>]
nullrouter catalogue check
```

- `list` shows each harness with its newest version and whether it is installed.
- `show` lists every version with its source URL, `sha256` and `source_fp`.
- `install` fetches the archive, checks its hash and fingerprint, and runs the gate. The version
  then follows the same review path as a local install. Without a semver, it takes the newest.
- `check` lists newer catalogue versions of installed harnesses. It installs nothing.

Limits on fetching:

| Limit | Value |
|---|---|
| Catalogue index | 1 MiB |
| Archive | 2 MiB |
| Redirects | 3 |
| Request timeout | 30 s |

The URL must use `https://`. A hash or fingerprint that doesn't match refuses the install.

Exit codes for `catalogue`:

| Code | Meaning |
|---|---|
| 1 | Any other error |
| 3 | Refused: a hash or fingerprint mismatch, a bad archive, or the gate |
| 5 | Could not fetch: a network error, a non-HTTPS URL, or a size over the limit |

## Upgrading the kit

The kit is the crate every adapter depends on. Its ABI number is `1` today. When the core moves
to a new kit ABI, a module built for the old ABI needs a rebuild before it runs:

1. Start the new `serve`. It rebuilds approved versions built for an ABI it no longer runs. The
   rebuild uses the reviewed source and the same source fingerprint, so no new review is needed.
2. If a rebuild fails, check `nullrouter alerts list` for `rebuild_failed`, and run
   `nullrouter adapters rebuild <harness>` after you fix the cause.
3. A new adapter version needs a review as usual. Use `catalogue check` to find one.
