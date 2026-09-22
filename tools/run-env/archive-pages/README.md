# Retired pages — kept, not deleted

**THE page is `alpine-guest/index.html`.** Everything here is a throwaway rig from
the measurement phase, kept because each one earned something that is now folded
into that page, and deleting them would throw away the record of what they proved.

**The failure this directory exists to stop:** six one-off pages in two days, each
rebuilt from scratch rather than extended, so work kept being re-done and lost.
`browser.html` was the worst of it — it reinvented a *worse* terminal than
`vm-app.html` already had, dropped the keyboard entirely, and put a JSON
diagnostics dump across 80% of the viewport. It was a measurement probe that got
mistaken for the product, correctly and angrily.

| page | what it proved | where that lives now |
|---|---|---|
| `v86-m1-index.html` | v86 boots with **no cross-origin isolation** (M1) | the `crossOriginIsolated` assertion in `alpine-browser-probe.py` |
| `v86-m1-demo.html` | first v86 boot in a browser at all | — |
| `v86-m1-roundtrip.html` | page→guest→page file round trip | the `x-put-files` / `x-get-file` verbs |
| `v86-m1-host.html` | the host side of the entity-apps handshake | still the reference host shape |
| `v86-m1-vm-app.html` | **the terminal, the key bar, mobile keyboard focus, `stty`, the file verbs** | ⭐ folded into `alpine-guest/index.html` — this is the page the current one descends from |
| `browser.html` | the transfer measurement (`transferSize` vs `encodedBodySize`) | `netStats()` in the one page, reported to `window.__m1` and never to the screen |

**Rule going forward: extend the one page.** If a measurement needs a surface, put
it behind `window.__m1` where a probe reads it — diagnostics belong to the probe,
never to the viewport.
