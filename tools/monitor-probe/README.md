# monitor-probe — what a page can read about its own load, measured

The research behind `docs/plans/DESIGN-2026-09-14-c-THE-SYSTEM-MONITOR-…`. One probe page collects
every performance/system API in five contexts (top page, dedicated worker, sandboxed `srcdoc` frame,
sandboxed same-origin-URL frame with `allow-same-origin`, sandboxed opaque-URL frame), runs a 300 ms
busy loop in each, and records what the TOP page can see of it.

Measured 2026-09-14 in Firefox 149.0.2 and Chrome 151.0.7922.108 (Selenium grids), plain and
cross-origin isolated (COEP `require-corp` and `credentialless`). `results/*.json` are the raw runs,
`summary.txt` is `summarize.py` over them. WebKit/WebKitGTK: **not measured**.

```sh
mkdir -p site/data && head -c 65536 /dev/urandom > site/data/blob.bin && cp site/data/blob.bin site/data/blob-tao.bin
python3 serve.py 8297 site plain &        # also: 8298 site corp, 8299 site credentialless
python3 drive.py firefox http://127.0.0.1:4480 http://127.0.0.1:8297/index.html results/firefox-plain.json
python3 summarize.py > summary.txt
```
