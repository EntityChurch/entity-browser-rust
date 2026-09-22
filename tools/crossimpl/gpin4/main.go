// gpin4-oracle runs the G-PIN-4 joint fixture through entity-workbench-go's
// OWN site types and entity-core-go's OWN trie, and reports every divergence
// against the values this repo computed.
//
// APP-CONVENTION-SEMANTIC-CONTENT-SITE §9 names a reproducible-publish check —
// "one fixture, two publishers, identical site root" — and says outright that
// "the fixtures, the bytes and the run are the implementations'". This program
// is the RUN. Until it existed, the only way to learn whether the two seats
// agreed was to send a packet and wait for a reply.
//
// WHAT IT IS NOT. It does not reimplement their encoder, their hash, or their
// trie — it CALLS them, exactly as `make crossimpl-go` consumes entity-core-go's
// own publisher rather than modelling one. Every value on the "workbench-go"
// side of the report comes out of their code. If this file ever grows a struct
// that mirrors one of theirs, the gate has stopped measuring anything.
//
// SCOPE, AND IT IS DELIBERATELY NARROW. It covers the two links the convention
// actually pins:
//
//	link 2 — site entity -> canonical bytes    (their entitysdk.SiteManifest / SitePage)
//	link 3 — binding set -> CHAMP root         (their core/tree.BuildTrie)
//
// It does NOT cover source-directory ingest. That is not an oversight: the
// convention specifies no authoring format, calls frontmatter "optional local
// flavor not the contract" (§0) and makes title-derivation a MAY (§4 CDDL), so
// two conformant publishers are PERMITTED to lower one markdown file to
// different entities. A comparison there measures a free choice, and a gate
// that reds on a free choice teaches the next reader to ignore it.
//
// A DIVERGENCE IS ROUTED, NOT CORRECTED. Both seats hold that rule for the
// other's vectors, and it is the whole reason this is a report rather than a
// fixer: correcting our side to match theirs without settling which is right
// converts a cohort disagreement into a silent divergence with our name on it.
//
//	go run ./tools/crossimpl/gpin4 -fixture tests/fixtures/gpin4-joint
package main

import (
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"github.com/fxamacker/cbor/v2"

	"go.entitychurch.org/entity-core-go/core/ecf"
	"go.entitychurch.org/entity-core-go/core/entity"
	"go.entitychurch.org/entity-core-go/core/hash"
	"go.entitychurch.org/entity-core-go/core/store"
	"go.entitychurch.org/entity-core-go/core/tree"

	"entity-workbench-go/entitysdk"
)

// minKeys is the anti-vacuity floor: did the fixture LOAD at all. A run that
// compared nothing would otherwise print a clean report, which is how this area
// went unmeasured for a month on both sides.
//
// It is deliberately 2 — a manifest and one page — and NOT the fixture's true
// key count. A floor set at the real count turns "one publisher emits a key the
// other does not" into a fatal load error, which is the single most interesting
// divergence this program can find, reported as a rig fault. Measured: dropping
// one page from site.json hit an 8-key floor and never reached the key-set
// report. The floor guards the rig; the key-set comparison guards the finding.
const minKeys = 2

// --- the fixture, as language-neutral authored input -------------------------
//
// These mirror site.json's SHAPE, not either implementation's types. Parsing
// JSON is not the thing under test; what is under test is what their types do
// with the values.

type navJSON struct {
	Label    string    `json:"label"`
	Target   string    `json:"target"`
	Children []navJSON `json:"children"`
}

type pageJSON struct {
	Slug        string            `json:"slug"`
	Format      string            `json:"format"`
	Frontmatter map[string]string `json:"frontmatter"`
	Body        string            `json:"body"`
}

type fixtureJSON struct {
	SiteID string            `json:"site_id"`
	Title  string            `json:"title"`
	Params map[string]string `json:"params"`
	Nav    []navJSON         `json:"nav"`
	Pages  []pageJSON        `json:"pages"`
}

type expectedJSON struct {
	Bindings map[string]string `json:"bindings"`
	SiteRoot string            `json:"site_root"`
}

// toNav lowers the fixture's nav into THEIR NavItem. Their `omitempty` on
// Target and Children is what makes the no-target section and the leaf-with-no
// -children cases meaningful; we pass the values through and let their tags
// decide, which is the point.
func toNav(in []navJSON) []entitysdk.NavItem {
	if len(in) == 0 {
		return nil
	}
	out := make([]entitysdk.NavItem, 0, len(in))
	for _, n := range in {
		out = append(out, entitysdk.NavItem{
			Label:    n.Label,
			Target:   n.Target,
			Children: toNav(n.Children),
		})
	}
	return out
}

// theirHash encodes through their ECF and computes the content hash their
// core computes, rendered in the same hex form our EXPECTED.json records.
func theirHash(entityType string, v any) (hash.Hash, []byte, error) {
	data, err := ecf.Encode(v)
	if err != nil {
		return hash.Hash{}, nil, err
	}
	h, err := hash.Compute(entityType, data)
	if err != nil {
		return hash.Hash{}, nil, err
	}
	return h, data, nil
}

type row struct {
	key      string
	kind     string
	expected string
	got      string
}

func (r row) agrees() bool { return r.expected == r.got }

func main() {
	fixtureDir := flag.String("fixture", "tests/fixtures/gpin4-joint", "the joint fixture directory")
	verbose := flag.Bool("v", false, "print the encoded bytes for every divergent key")
	flag.Parse()

	fxRaw, err := os.ReadFile(filepath.Join(*fixtureDir, "site.json"))
	if err != nil {
		fatal("read site.json: %v", err)
	}
	var fx fixtureJSON
	if err := json.Unmarshal(fxRaw, &fx); err != nil {
		fatal("parse site.json: %v", err)
	}

	expRaw, err := os.ReadFile(filepath.Join(*fixtureDir, "EXPECTED.json"))
	if err != nil {
		fatal("read EXPECTED.json: %v", err)
	}
	var exp expectedJSON
	if err := json.Unmarshal(expRaw, &exp); err != nil {
		fatal("parse EXPECTED.json: %v", err)
	}

	cs := store.NewMemoryContentStore()
	var rows []row
	var bindings []tree.Binding
	bodies := map[string][]byte{}

	// --- the manifest --------------------------------------------------------
	m := entitysdk.SiteManifest{
		SiteID: fx.SiteID,
		Title:  fx.Title,
		Nav:    toNav(fx.Nav),
		Params: fx.Params,
	}
	mh, mdata, err := theirHash(entitysdk.TypeSiteManifest, m)
	if err != nil {
		fatal("encode manifest through entitysdk: %v", err)
	}
	putOrDie(cs, entitysdk.TypeSiteManifest, mdata)
	rows = append(rows, row{"manifest", entitysdk.TypeSiteManifest, exp.Bindings["manifest"], hexOf(mh)})
	bindings = append(bindings, tree.Binding{Path: "manifest", Hash: mh})
	bodies["manifest"] = mdata

	// --- the pages -----------------------------------------------------------
	for _, p := range fx.Pages {
		page := entitysdk.SitePage{
			Format:      p.Format,
			Body:        p.Body,
			Frontmatter: p.Frontmatter,
		}
		// An empty frontmatter map and an absent one are the same wire shape on
		// their side (`omitempty`) and the fixture's `glossary` row exists to
		// pin exactly that. Pass it through rather than normalizing here, or the
		// driver decides the case instead of their struct tag.
		ph, pdata, err := theirHash(entitysdk.TypeSitePage, page)
		if err != nil {
			fatal("encode page %q through entitysdk: %v", p.Slug, err)
		}
		key := "pages/" + p.Slug
		putOrDie(cs, entitysdk.TypeSitePage, pdata)
		rows = append(rows, row{key, entitysdk.TypeSitePage, exp.Bindings[key], hexOf(ph)})
		bindings = append(bindings, tree.Binding{Path: key, Hash: ph})
		bodies[key] = pdata
	}

	sort.Slice(rows, func(i, j int) bool { return rows[i].key < rows[j].key })

	// --- link 3: the trie root ----------------------------------------------
	// Built in a store holding only what we just put, so every CHAMP node is
	// constructed from scratch rather than re-found.
	root, err := tree.BuildTrie(cs, bindings)
	if err != nil {
		fatal("build trie through core/tree: %v", err)
	}

	// --- the report ----------------------------------------------------------
	fmt.Printf("G-PIN-4 — one fixture, two publishers, one site root\n")
	fmt.Printf("  fixture     %s\n", *fixtureDir)
	fmt.Printf("  ours        entity-browser-rust  (EXPECTED.json, computed by src/content_site/format.rs)\n")
	fmt.Printf("  theirs      entity-workbench-go/entitysdk over entity-core-go/core\n\n")

	width := 0
	for _, r := range rows {
		if len(r.key) > width {
			width = len(r.key)
		}
	}

	diverged := 0
	missing := 0
	for _, r := range rows {
		switch {
		case r.expected == "":
			missing++
			fmt.Printf("  ??  %-*s  no expected value in EXPECTED.json — theirs %s\n", width, r.key, r.got)
		case r.agrees():
			fmt.Printf("  ok  %-*s  %s\n", width, r.key, r.got)
		default:
			diverged++
			fmt.Printf("  XX  %-*s  DIVERGES\n", width, r.key)
			fmt.Printf("      %-*s    ours   %s\n", width, "", r.expected)
			fmt.Printf("      %-*s    theirs %s\n", width, "", r.got)
			if *verbose {
				fmt.Printf("      %-*s    their bytes %s\n", width, "", hex.EncodeToString(bodies[r.key]))
			}
		}
	}

	// Anything EXPECTED.json names that the fixture did not produce is a
	// divergence in the other direction — a key one publisher emits and the
	// other does not. Reported separately, because "we disagree about a hash"
	// and "we disagree about which keys exist" are different findings.
	extra := 0
	seen := map[string]bool{}
	for _, r := range rows {
		seen[r.key] = true
	}
	keys := make([]string, 0, len(exp.Bindings))
	for k := range exp.Bindings {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		if !seen[k] {
			extra++
			fmt.Printf("  XX  %-*s  ours emits this key, theirs does not\n", width, k)
		}
	}

	// The root is counted apart from the keys. "one page's bytes differ" and
	// "the trie root differs" are not the same finding — a root that diverges
	// with every key agreeing means the two trie builders disagree, which is a
	// link-3 result, and folding it into the key count hides that.
	rootDiverged := false
	fmt.Printf("\n  --  %-*s\n", width, "site root")
	if hexOf(root) == exp.SiteRoot {
		fmt.Printf("  ok  %-*s  %s\n", width, "site_root", hexOf(root))
	} else {
		rootDiverged = true
		fmt.Printf("  XX  %-*s  DIVERGES\n", width, "site_root")
		fmt.Printf("      %-*s    ours   %s\n", width, "", exp.SiteRoot)
		fmt.Printf("      %-*s    theirs %s\n", width, "", hexOf(root))
	}

	fmt.Printf("\n")

	// Anti-vacuity before the verdict: a clean report over nothing is the
	// failure mode this whole gate exists to avoid.
	if len(rows) < minKeys {
		fatal("anti-vacuity: compared %d keys, fixture must carry at least %d — did site.json load?",
			len(rows), minKeys)
	}

	if diverged+missing+extra == 0 && !rootDiverged {
		fmt.Printf("AGREED — %d keys and the site root, byte-identical across two implementations\n", len(rows))
		fmt.Printf("         over two independent cores (entity-core-rust / entity-core-go).\n")
		return
	}
	fmt.Printf("DIVERGED — %d of %d keys, %d key-set difference(s), site root %s.\n",
		diverged, len(rows), extra+missing, map[bool]string{true: "DIVERGES", false: "agrees"}[rootDiverged])
	fmt.Printf("           ROUTE this, do not correct either side locally: neither reading is\n")
	fmt.Printf("           privileged until the two seats settle which is right.\n")
	os.Exit(1)
}

func hexOf(h hash.Hash) string { return hex.EncodeToString(h.Bytes()) }

func putOrDie(cs *store.MemoryContentStore, entityType string, data []byte) {
	e, err := entity.NewEntity(entityType, cbor.RawMessage(data))
	if err != nil {
		fatal("construct %s entity: %v", entityType, err)
	}
	if _, err := cs.Put(e); err != nil {
		fatal("put %s: %v", entityType, err)
	}
}

func fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "gpin4-oracle: "+format+"\n", args...)
	os.Exit(2)
}
