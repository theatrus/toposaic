# Update feeds

TopoSaic uses two public JSON files with separate jobs:

- `notice.json` controls the update notice shown in the app.
- `updater.json` tells Tauri where to find signed update payloads.

The app fetches both the site notice and GitHub's latest stable release. It
keeps the newer valid version. Tauri checks the signed updater feed on its own
and enables in-app install only when that feed offers the same version.

## Notice file

The site should publish this file at
`https://toposaic.com/releases/notice.json`:

```json
{
  "schema_version": 1,
  "version": "0.2.0",
  "release_url": "https://github.com/theatrus/toposaic/releases/tag/v0.2.0",
  "summary": "Improves terrain detail and update support.",
  "urgency": "normal",
  "minimum_supported_version": "0.1.0",
  "published_at": "2026-07-24T18:00:00Z"
}
```

`urgency` can be `normal`, `recommended`, or `required`. The app also marks
the notice as required when its installed version is older than
`minimum_supported_version`. This affects the notice text and color; it does
not force an install.

## Signed updater file

The site should publish `updater.json` at
`https://toposaic.com/releases/updater.json`. Each platform entry contains the
full text of its `.sig` file:

```json
{
  "version": "0.2.0",
  "notes": "Improves terrain detail and update support.",
  "pub_date": "2026-07-24T18:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "INLINE SIGNATURE",
      "url": "https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-windows-x64.exe"
    },
    "linux-x86_64": {
      "signature": "INLINE SIGNATURE",
      "url": "https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-linux-x86_64.AppImage"
    },
    "darwin-aarch64": {
      "signature": "INLINE SIGNATURE",
      "url": "https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-macos-aarch64.app.tar.gz"
    }
  }
}
```

Tagged builds create both files and attach them to the matching GitHub
release. Tauri tries the site copy first and the GitHub release asset if the
site returns a non-success status.

## Secret boundary

The update signer needs one stable key pair. Released apps contain the public
key. The build reads the private key from protected environment values named
`TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; it never writes them to a release file.
`TAURI_UPDATER_PUBLIC_KEY` supplies the public half while packaging.

The web publishing flow only needs the finished `notice.json`,
`updater.json`, signed payloads, and `.sig` files. Flotswarm can move those
public outputs into the TopoSaic web repository without giving that repository
the private signing key. Keep the stable private key in Flotswarm's secret
store and inject it only into the release build.
