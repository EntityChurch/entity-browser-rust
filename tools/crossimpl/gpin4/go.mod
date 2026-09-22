// The G-PIN-4 oracle's module. It lives in THIS repo and consumes the sibling
// seats read-only through `replace` — no file in entity-workbench-go or
// entity-core-go is written, staged or built into their trees. Same boundary
// `make crossimpl-go` draws, one layer in: we call their published interface,
// we do not vendor or mirror it.
//
// The paths are relative to this directory, so they resolve for anyone whose
// checkout has the three repos as siblings — which is the layout every seat on
// this box already has. `make crossimpl-site` hard-fails with a message when
// they are not there rather than falling back to something weaker.
module entity-browser-rust/tools/crossimpl/gpin4

go 1.25.0

require (
	entity-workbench-go/entitysdk v0.0.0
	github.com/fxamacker/cbor/v2 v2.9.0
	go.entitychurch.org/entity-core-go/core v0.9.0
)

require (
	github.com/cenkalti/backoff v2.2.1+incompatible // indirect
	github.com/cloudflare/circl v1.6.3 // indirect
	github.com/coder/websocket v1.8.15 // indirect
	github.com/dustin/go-humanize v1.0.1 // indirect
	github.com/fsnotify/fsnotify v1.9.0 // indirect
	github.com/google/uuid v1.6.0 // indirect
	github.com/grandcat/zeroconf v1.0.0 // indirect
	github.com/mattn/go-isatty v0.0.20 // indirect
	github.com/miekg/dns v1.1.27 // indirect
	github.com/mr-tron/base58 v1.2.0 // indirect
	github.com/ncruces/go-strftime v1.0.0 // indirect
	github.com/remyoudompheng/bigfft v0.0.0-20230129092748-24d4a6f8daec // indirect
	github.com/x448/float16 v0.8.4 // indirect
	go.entitychurch.org/entity-core-go/ext v0.8.0 // indirect
	golang.org/x/crypto v0.53.0 // indirect
	golang.org/x/net v0.55.0 // indirect
	golang.org/x/sys v0.46.0 // indirect
	golang.org/x/text v0.38.0 // indirect
	modernc.org/libc v1.72.0 // indirect
	modernc.org/mathutil v1.7.1 // indirect
	modernc.org/memory v1.11.0 // indirect
	modernc.org/sqlite v1.50.0 // indirect
)

replace (
	entity-workbench-go/entitysdk => ../../../../entity-workbench-go/entitysdk
	go.entitychurch.org/entity-core-go/core => ../../../../entity-core-go/core
	go.entitychurch.org/entity-core-go/ext => ../../../../entity-core-go/ext
)
