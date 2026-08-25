// program-dump: export the three workbench-go compute programs as fixture
// bundles for the browser (Rust) generic host.
//
// For each program it:
//  1. authors the program into a FRESH in-memory Go peer via the unchanged
//     workbench Author* function (zero per-program knowledge here beyond the
//     roster row: root, author fn, seed, tick count, input schedule);
//  2. walks the program root and records every authored entity
//     (path, type, raw canonical-CBOR data, content hash) — including the
//     step/projection `_expr/` IR DAG nodes;
//  3. replays the mount contract with the EXPORTED entitysdk surface
//     (seed: initial_state→state_path + input initial→path; per tick:
//     eval(step) → put(state_path) → for each output port with source:
//     eval(source) → put(path)) — the same loop workbench's Host.tickOnce
//     runs — recording the state and port content hashes per tick as the
//     cross-impl oracle the Rust host must reproduce;
//  4. writes one JSON bundle per program.
//
// The input schedule writes the SEED's field name (Snake `{"dir":v}`,
// Asteroids `{"keys":v}` — the field the step's Field() lookup reads).
// The historical `bits` vs `keys` EncodeKeySet drift is resolved upstream
// (workbench-go pinned KeySet/EncodeKeySet to `keys`, the incumbent wire
// field — RESPONSE-GENERIC-HOST-INPUT-DEVICE-MODEL §3); `keys` here matches.
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
	"go.entitychurch.org/entity-core-go/core/types"

	"entity-workbench-go/entitysdk"
	wb "entity-workbench-go/programs"
)

// DumpEntity is one authored entity, program-relative path, raw canonical
// CBOR data (hex), and the Go-side content hash string for verification.
type DumpEntity struct {
	Path string `json:"path"`
	Type string `json:"type"`
	Data string `json:"data_hex"`
	Hash string `json:"hash"`
}

// InputEvent is an oracle input write applied BEFORE the given tick runs.
type InputEvent struct {
	Tick int    `json:"tick"`
	Port string `json:"port"` // port name from the descriptor
	Type string `json:"type"`
	Data string `json:"data_hex"`
}

// TickOracle records the boundary hashes after tick i completes.
type TickOracle struct {
	StateHash  string            `json:"state_hash"`
	PortHashes map[string]string `json:"port_hashes,omitempty"`
}

type Bundle struct {
	Program        string       `json:"program"`
	Root           string       `json:"root"`
	// OriginPeer is the authoring peer's id — the namespace baked into the
	// IR's lookup/tree paths by the entitysdk builder (LookupTreeLocal
	// peer-qualifies at build time). A receiving host must materialize and
	// run the program under THIS namespace: the "program-relative paths
	// port unchanged" claim does not hold for the IR internals — a
	// cross-impl finding for workbench/arch (browser review F1 follow-up).
	OriginPeer     string       `json:"origin_peer"`
	DescriptorPath string       `json:"descriptor_path"`
	RngSeed        uint64       `json:"rng_seed"`
	Entities       []DumpEntity `json:"entities"`
	Inputs         []InputEvent `json:"inputs,omitempty"`
	Oracle         []TickOracle `json:"oracle"`
}

type program struct {
	name   string
	root   string
	seed   uint64
	ticks  int
	author func(*entitysdk.AppPeer, string, uint64) (string, error)
	inputs []scheduledInput
}

type scheduledInput struct {
	tick  int
	port  string
	field string
	value uint64
}

func main() {
	out := flag.String("out", ".", "output directory for bundle JSON files")
	flag.Parse()

	programs := []program{
		{
			name: "life", root: wb.LifeRoot, seed: 12345, ticks: 12,
			author: wb.AuthorLife,
		},
		{
			name: "snake", root: wb.SnakeRoot, seed: 12345, ticks: 12,
			author: wb.AuthorSnake,
			inputs: []scheduledInput{
				{tick: 4, port: "dir", field: "dir", value: wb.SnakeDown},
				{tick: 8, port: "dir", field: "dir", value: wb.SnakeLeft},
			},
		},
		{
			name: "asteroids", root: wb.AsteroidsRoot, seed: 12345, ticks: 16,
			author: wb.AuthorAsteroids,
			inputs: []scheduledInput{
				{tick: 2, port: "keys", field: "keys", value: 1 << 1},  // right
				{tick: 5, port: "keys", field: "keys", value: 1 << 2},  // thrust
				{tick: 9, port: "keys", field: "keys", value: 1 << 3},  // fire
				{tick: 12, port: "keys", field: "keys", value: 0},      // release
			},
		},
	}

	for _, p := range programs {
		b, err := dumpProgram(p)
		if err != nil {
			fmt.Fprintf(os.Stderr, "FAIL %s: %v\n", p.name, err)
			os.Exit(1)
		}
		path := filepath.Join(*out, p.name+".json")
		raw, err := json.MarshalIndent(b, "", " ")
		if err != nil {
			fmt.Fprintf(os.Stderr, "FAIL %s: marshal: %v\n", p.name, err)
			os.Exit(1)
		}
		if err := os.WriteFile(path, append(raw, '\n'), 0o644); err != nil {
			fmt.Fprintf(os.Stderr, "FAIL %s: write: %v\n", p.name, err)
			os.Exit(1)
		}
		fmt.Printf("%s: %d entities, %d oracle ticks -> %s\n",
			p.name, len(b.Entities), len(b.Oracle), path)
	}
}

func dumpProgram(p program) (*Bundle, error) {
	ap, err := entitysdk.CreatePeer(entitysdk.PeerConfig{})
	if err != nil {
		return nil, fmt.Errorf("CreatePeer: %w", err)
	}
	defer ap.Close()

	descPath, err := p.author(ap, p.root, p.seed)
	if err != nil {
		return nil, fmt.Errorf("author: %w", err)
	}

	// 2. Walk the authored artifacts BEFORE any mount/tick writes state.
	entities, err := walk(ap, p.root)
	if err != nil {
		return nil, fmt.Errorf("walk: %w", err)
	}

	// 3. Oracle replay of the mount contract.
	descEnt, ok, err := ap.Get(descPath)
	if err != nil || !ok {
		return nil, fmt.Errorf("get descriptor %s: ok=%v err=%v", descPath, ok, err)
	}
	desc, err := wb.DecodeDescriptor(descEnt)
	if err != nil {
		return nil, fmt.Errorf("decode descriptor: %w", err)
	}
	if desc.Shard != nil {
		return nil, fmt.Errorf("sharded program — out of POC fixture scope")
	}

	// Seed (F-E1): initial_state -> state_path; each input initial -> path.
	if err := copyEntity(ap, desc.InitialState, desc.StatePath); err != nil {
		return nil, fmt.Errorf("seed state: %w", err)
	}
	for _, port := range desc.InputPorts {
		if err := copyEntity(ap, port.Initial, port.Path); err != nil {
			return nil, fmt.Errorf("seed input %s: %w", port.Name, err)
		}
	}

	portByName := map[string]wb.ProgramPort{}
	for _, port := range desc.InputPorts {
		portByName[port.Name] = port
	}

	var inputs []InputEvent
	oracle := make([]TickOracle, 0, p.ticks)
	for tick := 0; tick < p.ticks; tick++ {
		for _, in := range p.inputs {
			if in.tick != tick {
				continue
			}
			port, ok := portByName[in.port]
			if !ok {
				return nil, fmt.Errorf("tick %d: no input port %q", tick, in.port)
			}
			raw, err := ecf.Encode(map[string]interface{}{in.field: in.value})
			if err != nil {
				return nil, fmt.Errorf("encode input: %w", err)
			}
			ent, err := entity.NewEntity(port.TypeRef, cbor.RawMessage(raw))
			if err != nil {
				return nil, fmt.Errorf("input entity: %w", err)
			}
			if _, err := ap.PutEntity(port.Path, ent); err != nil {
				return nil, fmt.Errorf("put input: %w", err)
			}
			inputs = append(inputs, InputEvent{
				Tick: tick, Port: in.port, Type: port.TypeRef,
				Data: hex.EncodeToString(raw),
			})
		}

		// eval(step) -> put(state_path)
		typ, data, err := eval(ap, desc.Step)
		if err != nil {
			return nil, fmt.Errorf("tick %d: step: %w", tick, err)
		}
		ent, err := entity.NewEntity(typ, data)
		if err != nil {
			return nil, fmt.Errorf("tick %d: state entity: %w", tick, err)
		}
		if _, err := ap.PutEntity(desc.StatePath, ent); err != nil {
			return nil, fmt.Errorf("tick %d: put state: %w", tick, err)
		}

		// Refresh source-bearing output ports (projections).
		portHashes := map[string]string{}
		for _, port := range desc.OutputPorts {
			if port.Source == "" {
				continue
			}
			ptyp, pdata, err := eval(ap, port.Source)
			if err != nil {
				return nil, fmt.Errorf("tick %d: port %s: %w", tick, port.Name, err)
			}
			if ptyp != port.TypeRef {
				return nil, fmt.Errorf("tick %d: port %s: type %s != declared %s",
					tick, port.Name, ptyp, port.TypeRef)
			}
			pent, err := entity.NewEntity(ptyp, pdata)
			if err != nil {
				return nil, fmt.Errorf("tick %d: port entity: %w", tick, err)
			}
			if _, err := ap.PutEntity(port.Path, pent); err != nil {
				return nil, fmt.Errorf("tick %d: put port: %w", tick, err)
			}
			portHashes[port.Name] = pent.ContentHash.String()
		}

		stateEnt, ok, err := ap.Get(desc.StatePath)
		if err != nil || !ok {
			return nil, fmt.Errorf("tick %d: read back state: ok=%v err=%v", tick, ok, err)
		}
		oracle = append(oracle, TickOracle{
			StateHash:  stateEnt.ContentHash.String(),
			PortHashes: portHashes,
		})
	}

	// Anti-vacuity (workbench's own guard): the program must actually evolve.
	distinct := map[string]bool{}
	for _, o := range oracle {
		distinct[o.StateHash] = true
	}
	if len(distinct) < p.ticks/2 {
		return nil, fmt.Errorf("VACUOUS oracle: only %d distinct states over %d ticks",
			len(distinct), p.ticks)
	}

	return &Bundle{
		Program:        p.name,
		Root:           p.root,
		OriginPeer:     ap.PeerID(),
		DescriptorPath: descPath,
		RngSeed:        p.seed,
		Entities:       entities,
		Inputs:         inputs,
		Oracle:         oracle,
	}, nil
}

// eval mirrors workbench Host.eval on the exported surface: op "eval" on
// system/compute, empty primitive/any params, expression path as the sole
// resource target. compute/error values arrive at HTTP 200 and are faults.
func eval(ap *entitysdk.AppPeer, path string) (string, cbor.RawMessage, error) {
	req, err := entitysdk.PrimitiveAny(map[string]interface{}{})
	if err != nil {
		return "", nil, err
	}
	resp, err := ap.Executor().ExecuteOnResource("system/compute", "eval", req,
		&types.ResourceTarget{Targets: []string{path}})
	if err != nil {
		return "", nil, fmt.Errorf("eval dispatch %s: %w", path, err)
	}
	if resp.Status != 200 {
		return "", nil, fmt.Errorf("eval %s: status %d (type=%s)", path, resp.Status, resp.Type)
	}
	if resp.Type == types.TypeComputeError {
		var ed types.ComputeErrorData
		_ = ecf.Decode(resp.Data, &ed)
		return "", nil, fmt.Errorf("eval %s: compute/error code=%s message=%q", path, ed.Code, ed.Message)
	}
	return resp.Type, resp.Data, nil
}

func copyEntity(ap *entitysdk.AppPeer, src, dst string) error {
	ent, ok, err := ap.Get(src)
	if err != nil || !ok {
		return fmt.Errorf("get %s: ok=%v err=%v", src, ok, err)
	}
	_, err = ap.PutEntity(dst, ent)
	return err
}

// walk collects every entity under prefix (recursive; a path can carry an
// entity AND have children — e.g. `step` + `step/_expr/...`).
func walk(ap *entitysdk.AppPeer, prefix string) ([]DumpEntity, error) {
	var out []DumpEntity
	seen := map[string]bool{}
	var rec func(string) error
	rec = func(path string) error {
		if !seen[path] {
			seen[path] = true
			ent, ok, err := ap.Get(path)
			if err != nil {
				return fmt.Errorf("get %s: %w", path, err)
			}
			if ok {
				out = append(out, DumpEntity{
					Path: path,
					Type: ent.Type,
					Data: hex.EncodeToString(ent.Data),
					Hash: ent.ContentHash.String(),
				})
			}
		}
		entries, err := ap.List(path)
		if err != nil {
			// A leaf with no listing is fine.
			return nil
		}
		for _, e := range entries {
			child := path + "/" + e.Name
			if err := rec(child); err != nil {
				return err
			}
		}
		return nil
	}
	if err := rec(prefix); err != nil {
		return nil, err
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Path < out[j].Path })
	return out, nil
}
