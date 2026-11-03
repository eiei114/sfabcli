# sfabcli

`sfab` is a small CLI for [Sketchfab](https://sketchfab.com/) that reuses an already logged-in Chrome
over the DevTools protocol, then calls Sketchfab's own web API from inside the page. No OAuth token,
no headless login flow, no cookie storage: it borrows your browser session and never prints cookies.

Same approach as [fabcli](https://github.com/eiei114/fabcli); the two share one resident Chrome
profile.

```console
$ sfab status
ok: Keisu（pro・ログイン済み）

$ sfab search "wooden barrel" --details --license cc0 --limit 5
1a2b...  cc0 · gltf · 12400f  Wooden Barrel  — someone

$ sfab plan <uid> --format glb
{
 "format": "glb",
 "textureMaxResolution": 1024,
 "sizeMB": "6.4MB",
 "license": { "slug": "cc0", "free": true },
 "attributionRequired": false
}

$ sfab download <uid> --format glb --out ./barrel --unzip
saved ./barrel/Wooden_Barrel.glb (6.4MB)
record ./barrel/SFAB_SOURCE_1a2b3c4d.json
```

## Requirements

- Node.js 22+ (uses the built-in `fetch` and `WebSocket` globals)
- A Chrome instance started with `--remote-debugging-port` and logged in to Sketchfab
- `curl` (downloading) and `unzip` (only with `--unzip`)

## Install

```console
git clone https://github.com/eiei114/sfabcli
cd sfabcli
npm link        # or: export PATH="$PWD:$PATH"
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
| `sfab download <uid\|url> [--format ...] [--tex ...] [--out DIR] [--unzip] [--only NAME]` | download |

Downloadable formats are glTF, GLB, USDZ and source archives. Sketchfab does not publish standalone
FBX or OBJ.

## Notes and caveats

- **`--license` needs `--details`.** Search results do not carry license data, so filtering happens
  client-side after an N+1 detail fetch; the command refuses to filter without it.
- The `/i/search` `license` query parameter does not work (measured), so filtering is local.
- When several texture resolutions exist, the default pick is the **smallest** — a 4K set is rarely
  wanted for game import. Use `--tex` to override.
- `--only NAME` selects by filename suffix from a source archive.
- Each download writes `SFAB_SOURCE_<uid8>.json` with author, profile URL, license slug, label,
  requirements, attribution flag, AI flag, format and size. When attribution is required the exact
  credit line is printed.
- Treat third-party archives as untrusted input: check CRCs, path traversal and expanded size before
  extracting. `--unzip` uses plain `unzip`.
- Sketchfab's web API is undocumented and may change without notice.

## See also

- [fabcli](https://github.com/eiei114/fabcli) — the same approach for the Fab marketplace

## License

MIT
