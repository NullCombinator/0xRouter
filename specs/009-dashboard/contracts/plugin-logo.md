# Contract: Plugin logo

For plugin authors (goes into `docs/plugins.md`) and for the generator.

## Declaring

```toml
schema = 2
id = "xai"
category = "apikey"
alias = "xai"
logo = "xai.png"
```

- Optional. A bare file name ending in `.png`: no `/`, `\` or `..`. Otherwise the plugin fails
  validation on `logo`, as for any bad field.
- Looked up in the `logos/` directory beside the plugin's set:

| Plugin set | Directory |
|---|---|
| bundled | `plugins/bundled/logos/` (embedded in the binary) |
| community | `plugins/community/logos/` (embedded) |
| user and installed | `<home>/plugins/logos/` |

## Checked at load (by the core)

| Check | Failure note (`check`: `note: logo ignored: <id>: <reason>`) |
|---|---|
| The file exists | `file not found: logos/<file>` |
| At most 65,536 bytes | `<n> KiB, over 64 KiB` |
| Starts with the PNG signature, then an `IHDR` chunk | `not a PNG` |
| Width and height each 1 to 256 | `<w> × <h> px, over 256 px` |

A failed logo is ignored. The plugin loads and serves as before, and the dashboard shows its text
icon. The check doesn't decode the image. SVG is never accepted.

## Served

`GET /logos/<content hash>/<provider id>.png` on the dashboard port, behind the dashboard cookie
like a page, `Content-Type: image/png`,
`X-Content-Type-Options: nosniff`, under the CSP's `img-src 'self'`.

## Shipped logos (generator)

- `generate.mjs` copies `ref/9router/public/providers/<id>.png`, or
  `tools/gen-bundled/seeds/logos/<id>.png` when an override exists, to
  `plugins/{bundled,community}/logos/<id>.png`, and writes `logo = "<id>.png"` into each generated
  community plugin. The bundled plugins get the line by hand.
- It applies the same four checks and exits with an error naming the file when one fails, so a
  new oversized logo in `ref/9router` needs an override before it ships.
- It writes `plugins/LOGOS.md`: one row per logo with its source path (or override) at the ref SHA.
- Overrides today: `crush`, `nebius`, `reka`, `siliconflow`, `kimchi`, each converted once with
  ImageMagick to a PNG within the limits; the command used is recorded in `LOGOS.md`.
