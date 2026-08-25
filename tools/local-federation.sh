#!/usr/bin/env bash
# Stand up the WHOLE naming chain locally: N published domains + one registry
# that names them, as static files. No live peer anywhere.
#
#   ./tools/local-federation.sh [OUT_DIR]        (default: dist-federation)
#
# NOT under `dist/`: `make wasm` runs trunk, which WIPES dist/. See the Makefile's
# `federation` target (audit F10) — publishing here and then building the app used
# to delete the federation silently.
#
# What it produces:
#
#   {OUT}/registry/{registry-peer}/…      the signed name→peer-id bindings
#   {OUT}/{slug}/{domain-peer}/…          each domain's own signed publish
#   {OUT}/MAPPING.txt                     name → peer-id → slug, and the pin
#
# A consumer needs exactly ONE string out of all this — the registry's peer-id.
# Everything else it learns. (For Ed25519 canonical form a peer-id EMBEDS its
# public key, so the peer-id IS the pin; there is no key to distribute.)
#
# EDIT THIS TABLE to change what gets seeded. Columns:
#   name                  the name a user types
#   slug                  the directory / site-id it publishes under
#   seed                  32-byte hex identity seed — STABLE, so the domain's
#                         peer-id (and therefore every binding to it) survives a
#                         republish. Generate real ones for a real deployment:
#                         `openssl rand -hex 32`. These are DEV seeds.
#   ingest                optional path to a content-team `render/` emit; empty
#                         publishes the bundled demo site set.
DOMAINS=(
  "entitychurch.org|foundation|a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1|"
  "protocol.entitychurch.org|protocol|a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2|"
  "docs.entitychurch.org|docs|a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3|"
  "lab.entitychurch.org|lab|a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4|"
)
# The registry's own identity. Deliberately NOT any domain's: a consumer pins a
# name-issuer and a content-publisher independently, and one key doing both jobs
# means trusting a name-issuer to also be the thing it names.
REGISTRY_SEED="b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0"
TTL_DAYS="${TTL_DAYS:-30}"
# Prefix put in front of each slug when publishing a domain's origin into its
# binding. Empty = the single-origin local shape (`/foundation`, `/protocol`, …),
# which is SAME-ORIGIN and therefore needs no CORS. A real deployment sets real
# hostnames — and then every fetch from one domain's app to another's content is
# CROSS-ORIGIN and the target MUST send `Access-Control-Allow-Origin`
# (RUNBOOK-CDN-BROWSER-DEPLOYMENT §2). You do not get to dodge both: same-origin
# instead has the mutable-endpoint cache-staleness problem (runbook §3).
ORIGIN_BASE="${ORIGIN_BASE:-}"

set -euo pipefail
OUT="${1:-dist-federation}"
BIN="${BIN:-cargo run --quiet --bin entity-browser --}"
MAP="$OUT/MAPPING.txt"
# A DEDICATED stamp, not the human-facing MAPPING.txt. Both would work as a
# marker until the day some other tool leaves a MAPPING.txt behind — or until a
# stale one survives a partial clean, which is exactly how the first version of
# this guard was fooled into authorizing a delete it should have refused.
STAMP="$OUT/.local-federation"

# Safety guard before the wholesale clean. This script `rm -rf`s its output dir,
# so it refuses one it did not make: absent, empty, or carrying our stamp.
# Learned the direct way — the make target first shipped reusing the Makefile's
# existing `OUT ?= dist/static-demo`, and cleaned another target's directory.
if [ -e "$OUT" ] && [ ! -f "$STAMP" ] && [ -n "$(ls -A "$OUT" 2>/dev/null)" ]; then
  echo "refusing to clean $OUT — it is not empty and carries no .local-federation" >&2
  echo "stamp, so this script did not produce it. Remove it yourself, or pick" >&2
  echo "another path: make federation FED_OUT=dist-somewhere-else" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT"
: > "$STAMP"
: > "$MAP"

# THE PIN, ASKED FOR BEFORE THE REGISTRY EXISTS. A domain's
# `/entity-deployment.json` seeds `name_registry_pin` so a user who types nothing
# still resolves names — but the registry can only be emitted after the domains
# have peer-ids, so the pin has to be known first. `registry --peer-id` answers
# from the seed alone, using the same derivation the emit will use; the
# alternatives were publishing every domain twice or re-deriving the id in bash,
# and a second derivation of an identity is exactly the kind that drifts.
REGISTRY_PEER=$($BIN registry --identity-seed="$REGISTRY_SEED" --peer-id) || {
  echo "could not derive the registry peer-id from REGISTRY_SEED" >&2; exit 1; }
# Where the registry itself is served. Same shape as a domain's origin.
#
# **The `/` default is load-bearing and used to be missing.** With an empty
# ORIGIN_BASE the origins came out as bare slugs (`foundation`, `registry`),
# which a browser resolves RELATIVE TO THE CURRENT PATH — so an app served at
# `/foundation/` fetching origin `registry` asks for `/foundation/registry/…`
# and gets a 404 that reads like a withholding origin. `expand_origin` only
# treats a value as same-origin-relative when it starts with `/`; anything else
# is a concrete origin. The comment on ORIGIN_BASE above always said
# `/foundation`; the code said `foundation`, and nothing in a browser had ever
# consumed the same-origin shape to notice (the e2e sets an absolute base).
REGISTRY_ORIGIN="${ORIGIN_BASE:-/}registry"

binds=()
echo "=== domains ==="
for row in "${DOMAINS[@]}"; do
  IFS='|' read -r name slug seed ingest <<<"$row"
  args=("$OUT/$slug" "--identity-seed=$seed")
  # Each domain ships a deployment config carrying the pin. All four carry the
  # SAME one, which is the point: the pin is keyed by registry peer-id, so four
  # sites seeding it converge on one rather than accumulating four.
  args+=("--deployment-config" "--registry-pin=$REGISTRY_PEER@$REGISTRY_ORIGIN")
  [ -n "$ingest" ] && args+=("--ingest=$ingest")
  # The publish prints its resolved peer-id; that id is what the registry binds
  # and what a consumer pins. Reading it back beats recomputing it here — one
  # source of truth for a derivation we would otherwise duplicate.
  out=$($BIN publish "${args[@]}" 2>&1) || { echo "$out" >&2; exit 1; }
  peer=$(printf '%s\n' "$out" | awk '/^  peer:/ {print $2}')
  if [ -z "$peer" ]; then
    echo "publish for $name ($slug) printed no peer-id:" >&2
    printf '%s\n' "$out" >&2
    exit 1
  fi
  printf '%-28s %-12s %s\n' "$name" "$slug" "$peer" | tee -a "$MAP"
  # NAME=PEER@ORIGIN — the binding carries an EXTENSION-NETWORK §6.5.3
  # `http-poll` transport profile, so a consumer that resolves this name learns
  # WHERE to fetch as well as WHO. Without the `@ORIGIN` half every origin has
  # to be pre-registered out of band and the registry buys you nothing
  # operationally. `ORIGIN_BASE` is empty here (one local origin, path-prefixed);
  # set it to the real scheme+host per domain for a cross-origin deployment.
  binds+=("--bind=$name=$peer@${ORIGIN_BASE:-/}$slug")
done

echo
echo "=== registry ==="
mkdir -p "$OUT/registry"
# ONE invocation — emitting twice would work (it is idempotent) but a second
# publish is a second chance for the two to disagree.
# `ISSUED_AT_MS` pins the clock so the emission is REPRODUCIBLE. `issued_at`
# rides every binding body, so it decides every binding hash and therefore the
# whole trie shape: unpinned, two runs over identical content produce different
# bytes, and a fixture cut from one of them can neither be regenerated nor
# checked. Unset ⇒ wall clock, which is right for a real deployment and wrong
# for a corpus other repos consume.
issued_at_args=()
[ -n "${ISSUED_AT_MS:-}" ] && issued_at_args=("--issued-at=$ISSUED_AT_MS")
reg_out=$($BIN registry "$OUT/registry" --identity-seed="$REGISTRY_SEED" \
  --ttl-days="$TTL_DAYS" "${issued_at_args[@]}" "${binds[@]}" 2>&1) || { echo "$reg_out" >&2; exit 1; }
printf '%s\n' "$reg_out"
# The emit prints the id it published under; the domains were seeded with the id
# `--peer-id` predicted. They come from the same derivation, so they cannot
# differ — but a silent disagreement here would seed every domain with a pin at
# a registry that does not exist, which resolves nothing and reads exactly like a
# withholding origin. Cheap to check, and the failure it catches is not.
EMITTED_PEER=$(printf '%s\n' "$reg_out" | awk '/registry peer:/ {print $3}')
if [ "$EMITTED_PEER" != "$REGISTRY_PEER" ]; then
  echo "the registry published under $EMITTED_PEER but the domains were seeded" >&2
  echo "with a pin at $REGISTRY_PEER — every name would resolve to nothing" >&2
  exit 1
fi

{
  echo
  echo "registry peer (THE PIN — the only thing a consumer holds a priori):"
  echo "  $REGISTRY_PEER"
  echo
  echo "each domain's /entity-deployment.json SEEDS that pin at $REGISTRY_ORIGIN,"
  echo "so a user who types nothing still resolves names. \`name pin\` overrides it."
  echo
  echo "resolution: name → registry's signed root → peer-id → THAT peer's signed root → page"
} | tee -a "$MAP"

# Prove what we just emitted rather than assuming it. `--verify` walks each
# signed root's closure and fails (exit 2) if the tree is not walkable — the
# failure mode where every pointer resolves and a pinned consumer still gets
# nothing.
echo
echo "=== verify ==="
fail=0
for row in "${DOMAINS[@]}"; do
  IFS='|' read -r name slug seed ingest <<<"$row"
  if $BIN publish "$OUT/$slug" --identity-seed="$seed" --verify >/dev/null 2>&1; then
    echo "  ok   $slug"
  else
    echo "  FAIL $slug"; fail=1
  fi
done
# THE REGISTRY TOO — it was missing here for four sessions (audit F1). It is the
# one tree the entire chain hangs from: a consumer pins this key and nothing
# else, so an unwalkable registry root means every name resolves to nothing while
# all four domains below it verify clean and the whole rig looks green.
#
# `registry --verify`, NOT `publish --verify`: the two verbs resolve different
# durable identities, so `publish` would look under the publisher peer-id for a
# tree the registry emit wrote under the registry one. It matters less here (the
# seed is explicit either way) than as the shape a reader copies. Verify-only —
# it returns before any emit. Measured: 13/13 pointers, signed root verifies,
# 0 missing from the closure.
if $BIN registry "$OUT/registry" --identity-seed="$REGISTRY_SEED" --verify >/dev/null 2>&1; then
  echo "  ok   registry"
else
  echo "  FAIL registry"; fail=1
fi
if [ "$fail" -ne 0 ]; then
  echo "verification failed — this tree is not safe to serve" >&2
  exit 2
fi

echo
echo "mapping written to $MAP"
echo
echo "serve it (SAME-ORIGIN shape — one host, path-prefixed, no CORS needed):"
echo "  (cd $OUT && python3 -m http.server 8099)"
echo "    registry origin: http://localhost:8099/registry"
echo "    domain origins:  http://localhost:8099/{slug}"
echo
echo "serve it (CROSS-ORIGIN shape — what real domains are):"
echo "  ./tools/cors-serve.py $OUT 8099"
echo "    python3 -m http.server sends NO CORS headers, so a cross-origin fetch"
echo "    fails in a browser even though curl is happy. That is the gap"
echo "    RUNBOOK-CDN-BROWSER-DEPLOYMENT exists for."
