# sfabcli

`sfab` is a small Rust CLI for [Sketchfab](https://sketchfab.com/). It reuses an already logged-in
Chrome over the DevTools protocol, then calls Sketchfab's own web API from inside the page. No OAuth
token, no headless login flow, no cookie storage: it borrows your browser session and never prints
cookies.

Same approach as [fabcli](https://github.com/eiei114/fabcli); the two share one resident Chrome
profile.

```console
$ sfab status
ok: Eisuke114 (basic, signed in)

$ sfab search barrel --details --license by --limit 3
1f088de099d24cea9d8bba2a3d067403  by · free  Fishhook barrel cactus  — Graphic solutions

$ sfab plan ef2024160ce640558614d2d34d1e3c47 --format glb
{
  "format": "glb",
  "textureMaxResolution": 1024,
  "sizeMB": "3.5MB",
  "license": { "slug": "by", "needsAttribution": true },
  "attributionRequired": true
}

$ sfab download ef2024160ce640558614d2d34d1e3c47 --format glb --out ./barrel
saved ./barrel/Barrel.glb (3.5MB)
record ./barrel/SFAB_SOURCE_ef202416.json
ATTRIBUTION REQUIRED: "Barrel" by DZs (https://sketchfab.com/3d-models/...) — CC Attribution
```

## Requirements

- Rust 1.89+ to build; the binary is self-contained
- A Chrome instance started with `--remote-debugging-port` and logged in to Sketchfab

## Install

```console
cargo install sfabcli   # installs the `sfab` binary
# or, from a checkout
cargo install --path .
cargo build --release   # target/release/sfab
```

The default CDP endpoint is `http://127.0.0.1:9223`; override it with `SFAB_CDP`.

## Commands

| command | what it does |
| --- | --- |
| `sfab login` | open the Sketchfab login page in the CDP browser |
| `sfab status` | confirm the session and account type |
| `sfab search <words> [--downloadable] [--license by\|by-sa\|by-nc\|cc0] [--animated] [--staffpicked] [--max-face N] [--sort relevance\|likes\|recent] [--limit N] [--details] [--json]` | search models |
| `sfab show <uid\|url>` | license, download type, face/vertex counts, textures, AI flag |
| `sfab plan <uid\|url> [--format glb\|gltf\|source\|usdz] [--tex 1024\|2048\|4096]` | print the download plan without downloading |
| `sfab archive-types <uid\|url>` | every format and texture variant with sizes |
| `sfab download <uid\|url> [--format ...] [--tex ...] [--out DIR] [--unzip]` | download |

Downloadable formats are glTF, GLB, USDZ and source archives. Sketchfab does not publish standalone
FBX or OBJ.

Each download writes `SFAB_SOURCE_<uid8>.json` with the author, profile URL, license slug, label,
requirements, attribution flag, AI flag, format and size. When attribution is required the exact
credit line is printed.

## Notes and caveats

- **`--license` needs `--details`.** Search results do not carry license data, so filtering happens
  after one detail request per result; the command refuses to filter without it.
- The `/i/search` `license` query parameter does nothing (measured), so filtering is local.
- When several texture resolutions exist, the default pick is the **smallest** — a 4K set is rarely
  wanted for game import. Use `--tex` to override.
- A model with `downloadType: notDownloadable`, or one whose download endpoint returns 403, is
  refused with the author's reason rather than a silent failure.
- Treat third-party archives as untrusted input. Extraction sanitises entry names, so an entry
  cannot escape the destination, but check the contents before use.
- Sketchfab's web API is undocumented and may change without notice.

## Development

```console
cargo test      # unit tests, no network
cargo clippy --all-targets
```

The CDP session is only exercised by hand, since it needs a logged-in browser.

## See also

- [fabcli](https://github.com/eiei114/fabcli) — the same approach for the Fab marketplace

## License

MIT
