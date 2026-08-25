## Downloads

| File | Platform |
|---|---|
| `entity-browser_<ver>_linux_x64.deb` / `.rpm` / `.AppImage` | Linux x86-64 |
| `entity-browser_<ver>_linux_arm64.deb` / `.rpm` / `.AppImage` | Linux ARM64 |
| `entity-browser_<ver>_macos_x64.dmg` | macOS, Intel |
| `entity-browser_<ver>_macos_arm64.dmg` | macOS, Apple silicon |
| `entity-browser_<ver>_windows_x64.msi` / `_setup.exe` | Windows x86-64 |
| `entity-browser_<ver>_web.tar.gz` | the browser build — unpack onto any static origin |

The AppImage needs FUSE on the host; on a FUSE-less system run it with
`--appimage-extract-and-run`.

Every file is also published **without the version in its name**, so these links
always point at the newest release and never need updating:

```
https://github.com/EntityChurch/entity-browser-rust/releases/latest/download/entity-browser_linux_x64.deb
https://github.com/EntityChurch/entity-browser-rust/releases/latest/download/entity-browser_windows_x64_setup.exe
https://github.com/EntityChurch/entity-browser-rust/releases/latest/download/entity-browser_macos_arm64.dmg
```

## Verifying what you downloaded

`checksums.txt` covers every artifact, and the signature is over that file — so
one signature check plus one checksum check covers the whole release.

```sh
cosign verify-blob checksums.txt \
  --signature checksums.txt.sig \
  --certificate checksums.txt.pem \
  --certificate-identity-regexp '^https://github.com/EntityChurch/entity-browser-rust/' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com

sha256sum --check checksums.txt
```

This is a Sigstore **keyless** signature: there is no long-lived private key,
and the identity it proves is *"the release workflow in this repository built
this"*, recorded in the public Rekor transparency log.

Each artifact also carries GitHub build provenance — which workflow, at which
commit, produced it:

```sh
gh attestation verify entity-browser_0.8.0_linux_x64.deb \
  --repo EntityChurch/entity-browser-rust
```

## Not yet signed at the OS level

The macOS and Windows artifacts are **not** notarized or Authenticode-signed —
we hold no Apple Developer ID or code-signing certificate yet.

**What you will see, and what to do:**

- **Windows** — SmartScreen shows *"Windows protected your PC"*. Click
  **More info → Run anyway**.
- **macOS** — Gatekeeper refuses to open the app. Open **System Settings →
  Privacy & Security**, find the blocked app, and choose **Open Anyway**.
- **Linux** — unaffected. There is no equivalent gate; the packages just install.

Those warnings are accurate — they mean "the OS cannot tell you who built this,"
which is exactly true. What we can offer instead is the cosign signature and the
build provenance above, both of which tie the file to a public, logged build.

## Building it yourself

Nothing here needs our CI. A release build is `make dist` on the machine you
want to build for — the same recipe this workflow runs; `make dist-web` for the
browser bundle. The host needs only `make` and `podman`. See the
[Developer Guide](https://github.com/EntityChurch/entity-browser-rust/blob/master/docs/architecture/guides/DEVELOPER-GUIDE.md).

<!-- Do NOT link docs/RELEASE-READINESS.md here: it is on the publish pipeline's
     scrub list and does not exist on the public tree, so the link 404s for
     exactly the reader this section is written for. Every link in this file must
     resolve against the PUBLISHED master, which carries 26 docs files, not ours. -->

