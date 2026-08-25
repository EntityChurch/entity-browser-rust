//! Asking the router for a door — PCP (RFC 6887) and NAT-PMP (RFC 6886).
//!
//! # What this is for, stated against the thing it changes
//!
//! `ANALYSIS-NAT-REACHABILITY…` §2a: **browser ↔ *reachable* native needs no
//! WebRTC, no signaling node and no STUN.** The browser opens a plain WebSocket
//! at this process's listener, which `e2e_worker` Phase 14/14b proves daily by
//! moving files over it. Every hard row in that analysis — reflectors, relays,
//! rendezvous — exists to work around one side having *no reachable address*.
//!
//! So the highest-leverage thing this desktop peer can do is **have one**. It
//! already binds `0.0.0.0:4041`; what it has never done is ask the router to
//! forward a port to it. That is the whole distance between *reachable on this
//! Wi-Fi* and *reachable from anywhere* — and for a Tori acting as the
//! [signaling node](crate::signaling_node), between *a rendezvous for this
//! room* and *a rendezvous for your friends*.
//!
//! # Why we speak the protocol rather than take a dependency
//!
//! NAT-PMP is a 12-byte request and a 16-byte reply over UDP; PCP is 60 and 60.
//! Both go to the default gateway on port 5351. The whole of both codecs is
//! below and is exhaustively testable against a fixture socket with no router
//! in the loop — the same call the rig's 30-line `stun_responder.py` makes.
//! Taking a crate here would add a supply-chain edge for something smaller than
//! the code that would configure it.
//!
//! **UPnP IGD is the one that would justify a dependency, and it is not here.**
//! IGD is SSDP discovery plus SOAP-over-HTTP plus XML parsing, and it is also
//! the protocol with the widest consumer-router support — so this module's
//! coverage is *"routers that speak PCP or NAT-PMP"*, which is a real subset and
//! not the majority. That gap is named in the buildout, not hidden here: a
//! router that speaks only IGD is reported as **no mapping offered**, which is
//! honest and useless in exactly the same way. IGD is a second backend behind
//! this same lease, not a rewrite.
//!
//! # The two rules that shape every refusal below
//!
//! **A mapping is a LEASE, not a fact.** It expires. This repo has already
//! learned the dual of that on the dialer side — *"a peer being offline is not
//! the same as its address being wrong, and a saved LAN address dies when the
//! machine changes networks"* — and the standing rule from the transport-profile
//! work is that **a wrong address is worse than no address**, because it is one
//! the ladder spends a dial timeout on. So the external address is withdrawn the
//! moment a renewal fails, never left standing on the strength of the last
//! success.
//!
//! **A successful mapping can still hand back an unreachable address.** Behind
//! carrier-grade NAT the gateway maps a port perfectly and reports an external
//! IP in `100.64/10` — a second NAT sits above it, and nobody on the internet
//! can reach that. Every field says *mapped*; the address is a lie. That is why
//! [`is_publishable`] exists and why it runs on the *result*: publishing it
//! would be the "publish on the 200, never on issue" discipline failing at one
//! layer lower, where the 200 is real and the address is not.

use std::net::{Ipv4Addr, SocketAddrV4};

/// The port both protocols are served on, by both RFCs.
pub const MAPPING_PORT: u16 = 5351;

/// What we ask for, and what we renew at half of. RFC 6886 §3.3 recommends
/// 7200s; we ask for an hour because a shorter lease is a shorter window in
/// which a *stale* mapping can point at a machine that has since gone away —
/// and renewal is two UDP packets.
pub const REQUESTED_LIFETIME_SECS: u32 = 3600;

/// IANA protocol number for TCP. Our listener is a WebSocket, so TCP is the
/// only thing worth mapping — mapping UDP as well would open a port nothing in
/// this process is listening on.
const PROTO_TCP: u8 = 6;

/// Environment override for the per-peer persisted setting. `1`/`true` forces
/// mapping on, `0`/`false` off, absent defers to what was persisted.
pub const ENV_ENABLE: &str = "ENTITY_BROWSER_PORT_MAPPING";

/// Whether this backend asks the router to forward its port.
///
/// **Off by default, and this one is a bigger default than the rendezvous
/// toggle beside it.** The listener already binds `0.0.0.0`, so it is reachable
/// from this LAN by design (phone pairing). A port map changes that audience
/// from *this network* to *the internet*, which is a decision only the person
/// running the machine can make. Same three-way shape as
/// `signaling_node::resolve_enabled`, deliberately: two toggles that resolve
/// differently is how one of them ends up meaning something nobody expects.
pub fn resolve_enabled(persisted: bool) -> bool {
    match std::env::var(ENV_ENABLE) {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
        Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
        // A value we do not recognise is not a vote.
        _ => persisted,
    }
}

// ---------------------------------------------------------------------------
// Is this address worth telling anyone about?
// ---------------------------------------------------------------------------

/// Would publishing this address help anyone reach us?
///
/// **This is the module's most important function and it runs on a SUCCESS.**
/// A gateway behind carrier-grade NAT answers a mapping request correctly and
/// hands back an address inside `100.64.0.0/10`; a gateway on the inner side of
/// a double-NAT hands back `192.168.x.x`. In both cases every protocol field
/// says the mapping worked, and it did — one layer up, another NAT drops
/// everything. An address like that is worse than none: it looks like an
/// answer, it gets stored, and every peer that tries it spends a dial timeout
/// discovering what we could have known here.
///
/// Deliberately a whitelist by exclusion of every non-global range rather than
/// a check for the few we have met. `Ipv4Addr::is_global` is still unstable, so
/// the ranges are spelled out; each one is a real way this has been observed to
/// fail, not a theoretical completeness exercise.
pub fn is_publishable(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !(ip.is_private()            // 10/8, 172.16/12, 192.168/16 — double NAT
        || ip.is_loopback()      // 127/8
        || ip.is_link_local()    // 169.254/16 — DHCP never completed
        || ip.is_broadcast()
        || ip.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24
        || ip.is_unspecified()   // 0.0.0.0 — "no mapping" wearing a success
        || (a == 100 && (64..128).contains(&b)) // 100.64/10 — CGNAT, the common one
        || (a == 192 && b == 0)  // 192.0.0/24 — IETF protocol assignments
        || (a == 198 && (18..20).contains(&b)) // 198.18/15 — benchmarking
        || a >= 224)             // 224/4 multicast, 240/4 reserved
}

// ---------------------------------------------------------------------------
// Which box do we ask?
// ---------------------------------------------------------------------------

/// Why we have no gateway to talk to. Separate variants because they are
/// different sentences to a user: one is *"this platform, not yet"*, one is
/// *"you are not on a network"*, and one is a bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    /// We have no route-table reader for this OS. **Not a failure of the
    /// network** — say so, or a macOS user reads "no gateway" as a router
    /// problem and goes looking at the wrong box.
    Unsupported(&'static str),
    /// The route table was readable and had no default route.
    NoDefaultRoute,
    Unreadable(String),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(os) => {
                write!(f, "no default-gateway reader for {os} yet")
            }
            Self::NoDefaultRoute => write!(f, "no default route — not on a network?"),
            Self::Unreadable(e) => write!(f, "could not read the route table: {e}"),
        }
    }
}

/// The default IPv4 gateway, which is the only address either RFC says to ask.
///
/// **Linux only today, and that is stated rather than smoothed over.** macOS
/// (`route -n get default` / `sysctl net.route`) and Windows (`GetBestRoute2`)
/// each need their own reader, and shipping an untested parser for a platform
/// this seat cannot run is how a feature reports "your router refused" on a
/// machine whose router was never asked. Buildout item 23a.
pub fn default_gateway_v4() -> Result<Ipv4Addr, GatewayError> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/net/route")
            .map_err(|e| GatewayError::Unreadable(e.to_string()))?;
        parse_proc_net_route(&text).ok_or(GatewayError::NoDefaultRoute)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(GatewayError::Unsupported(std::env::consts::OS))
    }
}

/// Pick the default route's gateway out of `/proc/net/route`.
///
/// The columns are tab-separated and the addresses are **hex in the host's
/// byte order**, which is the one thing here worth getting wrong: `0102A8C0` is
/// `192.168.2.1`, not `1.2.168.192`. Read via `to_le_bytes` rather than the
/// `.to_be()` byte-swap that is the usual idiom, because that idiom is a no-op
/// on a big-endian host and silently yields a reversed address there.
///
/// A default route is `Destination == 0` **and** `Mask == 0`; the lowest metric
/// wins, because a box with both Wi-Fi and a VPN has several and the kernel
/// picks by metric.
#[cfg(target_os = "linux")]
fn parse_proc_net_route(text: &str) -> Option<Ipv4Addr> {
    let mut best: Option<(u32, Ipv4Addr)> = None;
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        // Iface Destination Gateway Flags RefCnt Use Metric Mask …
        if f.len() < 8 {
            continue;
        }
        let hex = |s: &str| u32::from_str_radix(s, 16).ok();
        let (dest, gw, mask) = (hex(f[1])?, hex(f[2])?, hex(f[7])?);
        if dest != 0 || mask != 0 || gw == 0 {
            continue;
        }
        let metric: u32 = f[6].parse().unwrap_or(u32::MAX);
        let b = gw.to_le_bytes();
        let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
        if best.is_none_or(|(m, _)| metric < m) {
            best = Some((metric, ip));
        }
    }
    best.map(|(_, ip)| ip)
}

// ---------------------------------------------------------------------------
// NAT-PMP — RFC 6886
// ---------------------------------------------------------------------------

pub mod natpmp {
    use super::*;

    pub const VERSION: u8 = 0;
    pub const OP_MAP_TCP: u8 = 2;
    /// Responses carry the request's opcode with the high bit set (§3.3).
    pub const RESPONSE_BIT: u8 = 128;

    /// §3.3 request: version, opcode, 2 reserved, internal port, suggested
    /// external port, lifetime. 12 bytes, big-endian throughout.
    ///
    /// We suggest the **internal port as the external one**. A router that can
    /// honour it gives us a stable, guessable address across restarts; one that
    /// cannot picks its own and tells us, which we accept — the suggestion is a
    /// preference in the RFC, never a requirement, and refusing a different
    /// assignment would throw away a working door.
    pub fn encode_map_request(internal_port: u16, lifetime: u32) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = VERSION;
        b[1] = OP_MAP_TCP;
        // b[2..4] reserved, zero.
        b[4..6].copy_from_slice(&internal_port.to_be_bytes());
        b[6..8].copy_from_slice(&internal_port.to_be_bytes());
        b[8..12].copy_from_slice(&lifetime.to_be_bytes());
        b
    }

    /// The parts of a §3.3 response we act on.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MapResponse {
        pub internal_port: u16,
        pub external_port: u16,
        pub lifetime: u32,
    }

    /// Decode a §3.3 map response, or say why not.
    ///
    /// **The `internal_port` echo is checked by the caller, not here** — this
    /// returns it rather than validating it, because "the router answered about
    /// a different port" and "the router refused" are different diagnoses and
    /// only the caller knows what it asked for.
    pub fn decode_map_response(buf: &[u8]) -> Result<MapResponse, super::MapError> {
        if buf.len() < 16 {
            return Err(super::MapError::Malformed("NAT-PMP response shorter than 16 bytes"));
        }
        if buf[0] != VERSION {
            return Err(super::MapError::Malformed("NAT-PMP response with a foreign version"));
        }
        if buf[1] != OP_MAP_TCP + RESPONSE_BIT {
            return Err(super::MapError::Malformed("NAT-PMP response to a different opcode"));
        }
        let result = u16::from_be_bytes([buf[2], buf[3]]);
        if result != 0 {
            return Err(super::MapError::Refused {
                protocol: "NAT-PMP",
                code: result,
                meaning: result_meaning(result),
            });
        }
        Ok(MapResponse {
            internal_port: u16::from_be_bytes([buf[8], buf[9]]),
            external_port: u16::from_be_bytes([buf[10], buf[11]]),
            lifetime: u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]),
        })
    }

    /// §3.5's result codes. Rendered because "the router said 2" helps nobody,
    /// and 2 in particular ("not authorized") is the one a user can actually
    /// fix — it is what a router with UPnP/NAT-PMP switched off returns.
    pub fn result_meaning(code: u16) -> &'static str {
        match code {
            1 => "the router does not support this protocol version",
            2 => "the router refused — port mapping is probably switched off in its settings",
            3 => "the router has no external network",
            4 => "the router is out of mapping resources",
            5 => "the router does not support port mapping",
            _ => "unrecognised result code",
        }
    }

    /// §3.2 external-address request. Two bytes, and we send it only to
    /// *report* the address — the map response does not carry one.
    pub fn encode_external_address_request() -> [u8; 2] {
        [VERSION, 0]
    }

    /// §3.2 response: version, op 128, result, epoch, 4 address bytes.
    pub fn decode_external_address(buf: &[u8]) -> Result<Ipv4Addr, super::MapError> {
        if buf.len() < 12 {
            return Err(super::MapError::Malformed("NAT-PMP address response too short"));
        }
        if buf[1] != RESPONSE_BIT {
            return Err(super::MapError::Malformed("not an address response"));
        }
        let result = u16::from_be_bytes([buf[2], buf[3]]);
        if result != 0 {
            return Err(super::MapError::Refused {
                protocol: "NAT-PMP",
                code: result,
                meaning: result_meaning(result),
            });
        }
        Ok(Ipv4Addr::new(buf[8], buf[9], buf[10], buf[11]))
    }
}

// ---------------------------------------------------------------------------
// PCP — RFC 6887
// ---------------------------------------------------------------------------

pub mod pcp {
    use super::*;

    pub const VERSION: u8 = 2;
    pub const OP_MAP: u8 = 1;
    /// Set in the opcode byte of a response (§7.2).
    pub const RESPONSE_BIT: u8 = 0x80;
    pub const REQUEST_LEN: usize = 60;
    pub const RESPONSE_LEN: usize = 60;
    /// A MAP mapping is identified by this across renewals (§11.1). Keeping it
    /// stable is what makes a renewal a renewal rather than a second mapping.
    pub const NONCE_LEN: usize = 12;

    /// A v4 address as PCP carries it: IPv4-mapped IPv6 (§5). Every address
    /// field in the protocol is 16 bytes, so a v4-only client still writes 16.
    fn v4_mapped(ip: Ipv4Addr) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[10] = 0xff;
        b[11] = 0xff;
        b[12..16].copy_from_slice(&ip.octets());
        b
    }

    fn v4_from_mapped(b: &[u8]) -> Option<Ipv4Addr> {
        if b.len() < 16 {
            return None;
        }
        // Accept the v4-mapped form only. A server answering with a real IPv6
        // address has given us something this listener cannot be reached at
        // over v4, and treating its low 4 bytes as an address would invent one.
        if b[..10].iter().any(|&x| x != 0) || b[10] != 0xff || b[11] != 0xff {
            return None;
        }
        Some(Ipv4Addr::new(b[12], b[13], b[14], b[15]))
    }

    /// §11.1 MAP request: a 24-byte common header then 36 bytes of MAP payload.
    ///
    /// `client_ip` is the address of *this* host on the link to the gateway
    /// (§8.1) — the server checks it against the packet's source and answers
    /// `ADDRESS_MISMATCH` if they disagree, which is how a request sent from
    /// the wrong interface fails.
    pub fn encode_map_request(
        nonce: &[u8; NONCE_LEN],
        client_ip: Ipv4Addr,
        internal_port: u16,
        lifetime: u32,
    ) -> [u8; REQUEST_LEN] {
        let mut b = [0u8; REQUEST_LEN];
        b[0] = VERSION;
        b[1] = OP_MAP; // R bit clear = request
        // b[2..4] reserved
        b[4..8].copy_from_slice(&lifetime.to_be_bytes());
        b[8..24].copy_from_slice(&v4_mapped(client_ip));
        b[24..36].copy_from_slice(nonce);
        b[36] = PROTO_TCP;
        // b[37..40] reserved
        b[40..42].copy_from_slice(&internal_port.to_be_bytes());
        b[42..44].copy_from_slice(&internal_port.to_be_bytes()); // suggested external
        b[44..60].copy_from_slice(&v4_mapped(Ipv4Addr::UNSPECIFIED)); // "you choose"
        b
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MapResponse {
        pub internal_port: u16,
        pub external_port: u16,
        pub external_ip: Ipv4Addr,
        pub lifetime: u32,
    }

    /// Decode a §7.2 + §11.2 MAP response.
    ///
    /// **The nonce is checked here, and it is the one field that must be.** It
    /// is what ties a response to *our* mapping; an off-path attacker who can
    /// guess our source port but not the nonce can otherwise hand us a forged
    /// external address, which we would then publish. Everything else in this
    /// decoder is a shape check; this one is the security property.
    pub fn decode_map_response(
        buf: &[u8],
        expect_nonce: &[u8; NONCE_LEN],
    ) -> Result<MapResponse, super::MapError> {
        if buf.len() < RESPONSE_LEN {
            return Err(super::MapError::Malformed("PCP response shorter than 60 bytes"));
        }
        if buf[0] != VERSION {
            return Err(super::MapError::Malformed("PCP response with a foreign version"));
        }
        if buf[1] != OP_MAP | RESPONSE_BIT {
            return Err(super::MapError::Malformed("PCP response to a different opcode"));
        }
        let code = buf[3];
        if code != 0 {
            return Err(super::MapError::Refused {
                protocol: "PCP",
                code: code as u16,
                meaning: result_meaning(code),
            });
        }
        if &buf[24..36] != expect_nonce {
            return Err(super::MapError::Malformed(
                "PCP response carried a different mapping nonce — not an answer to our request",
            ));
        }
        let external_ip = v4_from_mapped(&buf[44..60])
            .ok_or(super::MapError::Malformed("PCP gave a non-IPv4 external address"))?;
        Ok(MapResponse {
            internal_port: u16::from_be_bytes([buf[40], buf[41]]),
            external_port: u16::from_be_bytes([buf[42], buf[43]]),
            external_ip,
            lifetime: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
        })
    }

    /// §7.4's result codes, in the words a user could act on.
    pub fn result_meaning(code: u8) -> &'static str {
        match code {
            1 => "the router does not support this protocol version",
            2 => "the router refused — port mapping is probably switched off in its settings",
            3 => "malformed request (ours)",
            4 => "the router does not support port mapping",
            5 | 6 => "the router rejected an option (ours)",
            7 => "the router has no external network",
            8 => "the router is out of mapping resources",
            9 => "the router does not map TCP",
            10 => "this host is over its mapping quota on the router",
            11 => "the router cannot provide an external address",
            12 => "the request came from a different address than it claimed",
            13 => "too many remote peers",
            _ => "unrecognised result code",
        }
    }

    /// A fresh 96-bit mapping nonce. Random because §11.2 makes it the only
    /// thing an off-path attacker cannot forge; `getrandom` rather than a
    /// counter or a clock for the same reason.
    pub fn fresh_nonce() -> [u8; NONCE_LEN] {
        let mut n = [0u8; NONCE_LEN];
        if getrandom::getrandom(&mut n).is_err() {
            // Falling back to a clock would be worse than saying so: a
            // predictable nonce is the one failure this field exists to
            // prevent. The caller treats an all-zero nonce as "do not use PCP".
            return [0u8; NONCE_LEN];
        }
        n
    }
}

// ---------------------------------------------------------------------------
// The exchange
// ---------------------------------------------------------------------------

/// Why we have no mapping. Every variant is a different sentence to a user and
/// a different next action — the whole reason this is not `Option`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// Nothing answered on 5351. **The overwhelmingly common case, and it is
    /// not an error condition** — it is what a router that speaks only UPnP
    /// IGD, or none of the three, looks like from here.
    NoResponse,
    /// The gateway answered and said no. `meaning` is the actionable half.
    Refused { protocol: &'static str, code: u16, meaning: &'static str },
    /// The gateway answered with something we could not read.
    Malformed(&'static str),
    /// **A mapping was granted and its address is not reachable from the
    /// internet** — carrier-grade NAT, or a second NAT above this one. Carries
    /// the address so the log can name it, because "your ISP has you behind
    /// CGNAT" is a completely different conversation from "your router said
    /// no", and the two are indistinguishable without it.
    NotPublishable(Ipv4Addr),
    /// The gateway mapped a port we did not ask about.
    WrongPort { asked: u16, answered: u16 },
    Io(String),
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoResponse => write!(
                f,
                "the router did not answer on port {MAPPING_PORT} \
                 (it may speak only UPnP IGD, which we do not yet)"
            ),
            Self::Refused { protocol, code, meaning } => {
                write!(f, "{protocol} refused ({code}): {meaning}")
            }
            Self::Malformed(m) => write!(f, "unreadable answer: {m}"),
            Self::NotPublishable(ip) => write!(
                f,
                "the router mapped a port but its external address is {ip}, which is not \
                 reachable from the internet — you are behind a second NAT (carrier-grade \
                 NAT); a port map here cannot help"
            ),
            Self::WrongPort { asked, answered } => {
                write!(f, "asked about port {asked}, the router answered about {answered}")
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// A door that is currently open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Where the world reaches us. Already checked by [`is_publishable`].
    pub external: SocketAddrV4,
    /// What the gateway granted — **not what we asked for**. Renewal is keyed
    /// off this, and a router that silently grants less than requested is
    /// common enough that using our own number would let mappings lapse.
    pub lifetime_secs: u32,
    /// Which protocol won, for the log and the UI.
    pub protocol: &'static str,
}

impl Mapping {
    /// When to renew: half the granted lifetime, RFC 6886 §3.3's advice, with a
    /// floor so a gateway granting a 2-second lease cannot spin us.
    pub fn renew_after_secs(&self) -> u64 {
        (self.lifetime_secs as u64 / 2).max(30)
    }
}

/// Retransmission schedule. RFC 6886 §3.1 says 250 ms doubling nine times,
/// which is 64 seconds — correct for a protocol that owns the boot, and far too
/// long for one whose failure is the *expected* case. Three tries is ~1.75 s per
/// protocol; the whole probe therefore costs under four seconds before it
/// reports "no mapping offered", and it never runs on the path that binds.
const RETRIES: &[u64] = &[250, 500, 1000];

/// Ask the gateway for a TCP mapping of `internal_port`, PCP first.
///
/// **PCP first, NAT-PMP second, and the order matters.** PCP is NAT-PMP's
/// successor and most PCP servers also answer version 0, so asking the older
/// one first would get an answer from a server whose better protocol we then
/// never use — and PCP is the one that reports the external address *in the map
/// response*, saving a second round trip. A PCP server that dislikes v2 says
/// `UNSUPP_VERSION`, which is our cue to drop to NAT-PMP; so is silence.
pub async fn request_mapping(
    gateway: Ipv4Addr,
    client_ip: Ipv4Addr,
    internal_port: u16,
    lifetime: u32,
    nonce: &[u8; pcp::NONCE_LEN],
) -> Result<Mapping, MapError> {
    request_mapping_at(
        SocketAddrV4::new(gateway, MAPPING_PORT),
        client_ip,
        internal_port,
        lifetime,
        nonce,
    )
    .await
}

/// [`request_mapping`] with the target spelled out, so a test can point it at a
/// fixture socket on loopback instead of a gateway on 5351.
///
/// **The whole protocol sequence lives here and nowhere else.** The first draft
/// of this module had the production path and the testable path as two copies
/// of the same sequence — which is the arrangement where the gate passes while
/// production sends something else, and this repo has already paid for that
/// shape twice (`pack_mirror` against the tree, the two publish emitters). The
/// only difference a test is allowed is *where the packets go*.
pub async fn request_mapping_at(
    target: SocketAddrV4,
    client_ip: Ipv4Addr,
    internal_port: u16,
    lifetime: u32,
    nonce: &[u8; pcp::NONCE_LEN],
) -> Result<Mapping, MapError> {
    // --- PCP ---------------------------------------------------------------
    // An all-zero nonce means `fresh_nonce` could not get randomness, and a
    // predictable nonce is the one thing §11.2 exists to prevent — so we skip
    // PCP rather than send a forgeable request.
    if nonce.iter().any(|&b| b != 0) {
        let req = pcp::encode_map_request(nonce, client_ip, internal_port, lifetime);
        match exchange(target, &req, pcp::RESPONSE_LEN).await {
            Ok(buf) => match pcp::decode_map_response(&buf, nonce) {
                Ok(r) => {
                    return finish(
                        r.external_ip,
                        r.internal_port,
                        r.external_port,
                        r.lifetime,
                        internal_port,
                        "PCP",
                    )
                }
                // Only an explicit "wrong version" falls through to NAT-PMP. A
                // refusal for any other reason is an ANSWER, and retrying it in
                // an older dialect would turn one clear "port mapping is off"
                // into two confusing ones.
                Err(MapError::Refused { code: 1, .. }) => {}
                Err(e) => return Err(e),
            },
            Err(MapError::NoResponse) => {}
            Err(e) => return Err(e),
        }
    }

    // --- NAT-PMP -----------------------------------------------------------
    let req = natpmp::encode_map_request(internal_port, lifetime);
    let buf = exchange(target, &req, 16).await?;
    let r = natpmp::decode_map_response(&buf)?;

    // NAT-PMP's map response carries no address, so we ask for it separately
    // (§3.2). A mapping we cannot name an address for is not publishable, so a
    // failure here fails the whole request rather than reporting a half-answer.
    let addr_buf = exchange(target, &natpmp::encode_external_address_request(), 12).await?;
    let external_ip = natpmp::decode_external_address(&addr_buf)?;

    finish(external_ip, r.internal_port, r.external_port, r.lifetime, internal_port, "NAT-PMP")
}

/// The checks both protocols share, applied to whatever they returned.
fn finish(
    external_ip: Ipv4Addr,
    echoed_internal: u16,
    external_port: u16,
    lifetime: u32,
    asked: u16,
    protocol: &'static str,
) -> Result<Mapping, MapError> {
    if echoed_internal != asked {
        return Err(MapError::WrongPort { asked, answered: echoed_internal });
    }
    // A zero lifetime is how both protocols spell a DELETED mapping. Treating
    // it as success would publish an address the gateway has just told us it is
    // no longer forwarding.
    if lifetime == 0 {
        return Err(MapError::Malformed("the router granted a zero-second lifetime"));
    }
    if !is_publishable(external_ip) {
        return Err(MapError::NotPublishable(external_ip));
    }
    Ok(Mapping {
        external: SocketAddrV4::new(external_ip, external_port),
        lifetime_secs: lifetime,
        protocol,
    })
}

/// One request/response over UDP with the retransmit schedule.
///
/// Binds an ephemeral port and `connect`s it to the gateway, so the kernel
/// drops replies from anywhere else — the cheap half of not believing an
/// off-path answer. (PCP's nonce is the half that matters; NAT-PMP has none,
/// which is one more reason to prefer PCP.)
async fn exchange(
    target: SocketAddrV4,
    req: &[u8],
    expect_len: usize,
) -> Result<Vec<u8>, MapError> {
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| MapError::Io(e.to_string()))?;
    sock.connect(target).await.map_err(|e| MapError::Io(e.to_string()))?;

    let mut buf = vec![0u8; 1024];
    for wait_ms in RETRIES {
        if let Err(e) = sock.send(req).await {
            return Err(MapError::Io(e.to_string()));
        }
        let deadline = std::time::Duration::from_millis(*wait_ms);
        match tokio::time::timeout(deadline, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if n >= expect_len => {
                buf.truncate(n);
                return Ok(buf);
            }
            // A short datagram is not an answer; keep waiting out this attempt
            // rather than treating a stray packet as a malformed reply.
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => return Err(MapError::Io(e.to_string())),
            Err(_) => continue, // timed out — retransmit
        }
    }
    Err(MapError::NoResponse)
}

/// Release a mapping: the same request with a **zero lifetime**, which is how
/// both RFCs spell deletion (RFC 6886 §3.4, RFC 6887 §15).
///
/// Best-effort and deliberately un-retried: we run this on shutdown, the
/// mapping expires on its own within the hour anyway, and blocking a quit on a
/// router that has stopped answering would be a worse bug than a lingering
/// port. Failure is logged by the caller, not propagated.
pub async fn release_mapping(
    gateway: Ipv4Addr,
    client_ip: Ipv4Addr,
    internal_port: u16,
    nonce: &[u8; pcp::NONCE_LEN],
) {
    let target = SocketAddrV4::new(gateway, MAPPING_PORT);
    if nonce.iter().any(|&b| b != 0) {
        let req = pcp::encode_map_request(nonce, client_ip, internal_port, 0);
        let _ = exchange(target, &req, pcp::RESPONSE_LEN).await;
    }
    let req = natpmp::encode_map_request(internal_port, 0);
    let _ = exchange(target, &req, 16).await;
}

// ---------------------------------------------------------------------------
// The lease
// ---------------------------------------------------------------------------

/// What the door looks like right now. Every variant is something the UI says
/// differently, which is the reason this is not `Option<SocketAddrV4>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapState {
    /// The user has not asked for this. **Not a failure**, and it must not read
    /// like one — the default posture is off.
    Disabled,
    /// Asked, no answer yet. Bounded by the retransmit schedule.
    Probing,
    /// A door is open. `external` is already `is_publishable`-checked.
    Open { external: SocketAddrV4, protocol: &'static str },
    /// No door, and this is the sentence explaining why — already written for a
    /// person (see [`MapError`]'s `Display` and [`GatewayError`]'s).
    Closed(String),
}

impl MapState {
    /// The address to publish, if there is one. **`None` for every other
    /// variant including `Probing`** — a caller must not be able to reach for
    /// "the last address we had", which is the whole staleness rule.
    pub fn external(&self) -> Option<SocketAddrV4> {
        match self {
            Self::Open { external, .. } => Some(*external),
            _ => None,
        }
    }
}

/// A live port-mapping lease: a background task that renews, plus the state it
/// publishes.
///
/// Cloneable and cheap — the clone shares the state, so the reporting path and
/// the maintain loop cannot disagree about what is open.
#[derive(Clone)]
pub struct Lease {
    state: std::sync::Arc<std::sync::Mutex<MapState>>,
    /// Kept so shutdown can send the zero-lifetime delete for the *same*
    /// mapping. A fresh nonce would ask the router to delete a mapping it has
    /// never heard of.
    nonce: [u8; pcp::NONCE_LEN],
    gateway: Ipv4Addr,
    client_ip: Ipv4Addr,
    internal_port: u16,
}

/// How long to wait before asking again after a failure.
///
/// Deliberately long. The overwhelmingly common failure is *"this router does
/// not speak either protocol"*, which will still be true in ten minutes, and
/// the probe costs the router two packets it does not understand. Short enough
/// that a router rebooting, or a laptop rejoining a network, recovers without a
/// restart — there is no network-change signal here to do better (buildout 23b).
const RETRY_AFTER_FAILURE_SECS: u64 = 600;

impl Lease {
    /// What the door looks like right now.
    pub fn state(&self) -> MapState {
        self.state.lock().map(|g| g.clone()).unwrap_or(MapState::Disabled)
    }

    /// Start asking, and keep the mapping alive.
    ///
    /// **Renewal is the point, and withdrawal on failure is the invariant.**
    /// A mapping is a lease: it expires, the router reboots, the laptop changes
    /// networks. When a renewal fails the address is dropped rather than left
    /// standing on the strength of the last success — this repo's transport
    /// profiles already carry the dual of that lesson (*"a wrong address is
    /// worse than no address, because it is one the ladder spends a dial
    /// timeout on"*), and a lapsed port map is exactly a wrong address.
    pub fn spawn(gateway: Ipv4Addr, client_ip: Ipv4Addr, internal_port: u16) -> (Self, tokio::task::JoinHandle<()>) {
        let lease = Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(MapState::Probing)),
            nonce: pcp::fresh_nonce(),
            gateway,
            client_ip,
            internal_port,
        };
        let task = {
            let l = lease.clone();
            tokio::spawn(async move {
                loop {
                    let wait = match request_mapping(
                        l.gateway,
                        l.client_ip,
                        l.internal_port,
                        REQUESTED_LIFETIME_SECS,
                        &l.nonce,
                    )
                    .await
                    {
                        Ok(m) => {
                            log::info!(
                                "port mapping: {} forwards {} to this peer's port {} ({} s lease)",
                                l.gateway,
                                m.external,
                                l.internal_port,
                                m.lifetime_secs
                            );
                            let after = m.renew_after_secs();
                            l.set(MapState::Open { external: m.external, protocol: m.protocol });
                            after
                        }
                        Err(e) => {
                            // Log at info, not warn: on the majority of routers
                            // this is the ordinary outcome, and a warning per
                            // ten minutes for the expected case trains people
                            // to ignore the log.
                            log::info!("port mapping: no external door — {e}");
                            l.set(MapState::Closed(e.to_string()));
                            RETRY_AFTER_FAILURE_SECS
                        }
                    };
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                }
            })
        };
        (lease, task)
    }

    fn set(&self, s: MapState) {
        if let Ok(mut g) = self.state.lock() {
            *g = s;
        }
    }

    /// Ask the router to close the door, best-effort, and mark it closed here
    /// immediately.
    ///
    /// Detached deliberately: shutdown must not block on a router that has
    /// stopped answering, and a missed delete costs at most one lease — the
    /// mapping expires on its own. What must NOT be missed is the local state
    /// change, so that happens synchronously, before the packet is even sent.
    pub fn release(&self) {
        self.set(MapState::Closed("released".into()));
        let (gw, ip, port, nonce) =
            (self.gateway, self.client_ip, self.internal_port, self.nonce);
        tokio::spawn(async move {
            release_mapping(gw, ip, port, &nonce).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // The check that runs on a success
    // -----------------------------------------------------------------------

    /// **CGNAT is the case this exists for.** A gateway behind carrier-grade
    /// NAT maps the port correctly and returns `100.64.0.0/10` — every field
    /// says success and nobody can reach it. Publishing that address would cost
    /// every peer a dial timeout to discover what one comparison knows here.
    #[test]
    fn an_address_nobody_can_reach_is_not_a_result() {
        assert!(!is_publishable(Ipv4Addr::new(100, 64, 0, 1)), "CGNAT, low edge");
        assert!(!is_publishable(Ipv4Addr::new(100, 127, 255, 254)), "CGNAT, high edge");
        // …and the addresses either side of that /10 ARE ordinary public space.
        assert!(is_publishable(Ipv4Addr::new(100, 63, 255, 255)));
        assert!(is_publishable(Ipv4Addr::new(100, 128, 0, 1)));

        // Double NAT: the gateway is itself on a private network.
        assert!(!is_publishable(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(!is_publishable(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!is_publishable(Ipv4Addr::new(172, 16, 0, 1)));
        // DHCP never completed.
        assert!(!is_publishable(Ipv4Addr::new(169, 254, 1, 1)));
        // "No mapping" wearing a success.
        assert!(!is_publishable(Ipv4Addr::UNSPECIFIED));
        assert!(!is_publishable(Ipv4Addr::LOCALHOST));
        assert!(!is_publishable(Ipv4Addr::new(224, 0, 0, 1)), "multicast");
        assert!(!is_publishable(Ipv4Addr::new(203, 0, 113, 5)), "documentation range");

        // The whole point: a real address passes.
        assert!(is_publishable(Ipv4Addr::new(81, 2, 69, 142)));
        assert!(is_publishable(Ipv4Addr::new(8, 8, 8, 8)));
    }

    // -----------------------------------------------------------------------
    // Gateway discovery
    // -----------------------------------------------------------------------

    /// The byte order is the trap. `0102A8C0` is `192.168.2.1`; a reader that
    /// takes the hex big-endian gets `1.2.168.192`, which is a real routable
    /// address somewhere and would be asked for a port map in silence.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_default_route_is_read_little_endian_and_by_metric() {
        let table = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp3s0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
wlp3s0\t0002A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
";
        assert_eq!(
            parse_proc_net_route(table),
            Some(Ipv4Addr::new(192, 168, 2, 1)),
            "0102A8C0 is 192.168.2.1 — little-endian, not 1.2.168.192"
        );

        // Two default routes (Wi-Fi + a VPN): the kernel picks by metric and so
        // must we, or we ask the wrong box and read its refusal as the answer.
        let two = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp3s0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
tun0\t00000000\t0100000A\t0003\t0\t0\t50\t00000000\t0\t0\t0
";
        assert_eq!(parse_proc_net_route(two), Some(Ipv4Addr::new(10, 0, 0, 1)), "lowest metric wins");

        // A non-default route must never be mistaken for one: destination zero
        // AND mask zero is the definition, and a host route to 0.0.0.0/32 is
        // not a default route.
        let none = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp3s0\t0002A8C0\t0102A8C0\t0003\t0\t0\t600\t00FFFFFF\t0\t0\t0
";
        assert_eq!(parse_proc_net_route(none), None);
        assert_eq!(parse_proc_net_route(""), None);
        assert_eq!(parse_proc_net_route("garbage\nnot\ta\troute\n"), None);
    }

    // -----------------------------------------------------------------------
    // Codecs — byte for byte, because a router will not tell us we got it wrong
    // -----------------------------------------------------------------------

    #[test]
    fn the_natpmp_request_is_twelve_bytes_in_the_rfcs_order() {
        let b = natpmp::encode_map_request(4041, 3600);
        assert_eq!(b.len(), 12);
        assert_eq!(b[0], 0, "version 0");
        assert_eq!(b[1], 2, "opcode 2 = map TCP");
        assert_eq!(&b[2..4], &[0, 0], "reserved");
        assert_eq!(&b[4..6], &4041u16.to_be_bytes(), "internal port");
        assert_eq!(&b[6..8], &4041u16.to_be_bytes(), "we suggest the same external port");
        assert_eq!(&b[8..12], &3600u32.to_be_bytes(), "lifetime");
    }

    #[test]
    fn a_natpmp_response_decodes_and_a_refusal_carries_its_reason() {
        let mut r = [0u8; 16];
        r[1] = 2 + 128;
        r[8..10].copy_from_slice(&4041u16.to_be_bytes());
        r[10..12].copy_from_slice(&50000u16.to_be_bytes());
        r[12..16].copy_from_slice(&7200u32.to_be_bytes());
        assert_eq!(
            natpmp::decode_map_response(&r),
            Ok(natpmp::MapResponse { internal_port: 4041, external_port: 50000, lifetime: 7200 })
        );

        // Result 2 is the one a user can act on — the router has port mapping
        // switched off — so it must reach them as that sentence, not as "2".
        let mut refused = r;
        refused[2..4].copy_from_slice(&2u16.to_be_bytes());
        match natpmp::decode_map_response(&refused) {
            Err(MapError::Refused { code: 2, meaning, .. }) => {
                assert!(meaning.contains("switched off"), "got: {meaning}")
            }
            other => panic!("expected a refusal carrying its reason, got {other:?}"),
        }

        // Shape checks: a truncated datagram, a foreign version, and an answer
        // to a different opcode are all "not our answer", never a mapping.
        assert!(natpmp::decode_map_response(&r[..15]).is_err());
        let mut wrong_ver = r;
        wrong_ver[0] = 1;
        assert!(natpmp::decode_map_response(&wrong_ver).is_err());
        let mut wrong_op = r;
        wrong_op[1] = 1 + 128;
        assert!(natpmp::decode_map_response(&wrong_op).is_err());
    }

    #[test]
    fn the_pcp_request_is_sixty_bytes_with_the_client_address_v4_mapped() {
        let nonce = [7u8; 12];
        let b = pcp::encode_map_request(&nonce, Ipv4Addr::new(192, 168, 1, 22), 4041, 3600);
        assert_eq!(b.len(), 60);
        assert_eq!(b[0], 2, "version 2");
        assert_eq!(b[1], 1, "MAP, R bit clear");
        assert_eq!(&b[4..8], &3600u32.to_be_bytes());
        // §5: every address field is 16 bytes, v4 written as ::ffff:a.b.c.d.
        assert_eq!(&b[8..24], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 192, 168, 1, 22]);
        assert_eq!(&b[24..36], &nonce);
        assert_eq!(b[36], 6, "TCP");
        assert_eq!(&b[40..42], &4041u16.to_be_bytes());
        assert_eq!(&b[42..44], &4041u16.to_be_bytes());
        assert_eq!(&b[56..60], &[0, 0, 0, 0], "no suggested external address");
    }

    fn pcp_ok(nonce: &[u8; 12], external: Ipv4Addr, port: u16) -> [u8; 60] {
        let mut r = [0u8; 60];
        r[0] = 2;
        r[1] = 1 | 0x80;
        r[4..8].copy_from_slice(&1800u32.to_be_bytes());
        r[24..36].copy_from_slice(nonce);
        r[36] = 6;
        r[40..42].copy_from_slice(&4041u16.to_be_bytes());
        r[42..44].copy_from_slice(&port.to_be_bytes());
        r[54] = 0xff;
        r[55] = 0xff;
        r[56..60].copy_from_slice(&external.octets());
        r
    }

    /// **The nonce check is the security property in this module.** NAT-PMP has
    /// no equivalent, so an off-path attacker who guesses our source port can
    /// hand us an external address we would then publish as our own. PCP's
    /// nonce is what makes that forgery need a 96-bit guess.
    #[test]
    fn a_pcp_response_carrying_someone_elses_nonce_is_not_our_mapping() {
        let nonce = [9u8; 12];
        let good = pcp_ok(&nonce, Ipv4Addr::new(81, 2, 69, 142), 50000);
        assert_eq!(
            pcp::decode_map_response(&good, &nonce),
            Ok(pcp::MapResponse {
                internal_port: 4041,
                external_port: 50000,
                external_ip: Ipv4Addr::new(81, 2, 69, 142),
                lifetime: 1800,
            })
        );

        // Same packet, different nonce: refused, and NOT as a mapping.
        let other = [1u8; 12];
        assert!(matches!(
            pcp::decode_map_response(&good, &other),
            Err(MapError::Malformed(_))
        ));
    }

    #[test]
    fn a_pcp_refusal_names_the_fixable_case() {
        let nonce = [3u8; 12];
        let mut r = pcp_ok(&nonce, Ipv4Addr::new(81, 2, 69, 142), 50000);
        r[3] = 2; // NOT_AUTHORIZED
        match pcp::decode_map_response(&r, &nonce) {
            Err(MapError::Refused { protocol: "PCP", code: 2, meaning }) => {
                assert!(meaning.contains("switched off"), "got: {meaning}")
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    /// A server answering with a real IPv6 external address has given us
    /// something this v4 listener cannot be reached at. Taking its low four
    /// bytes would invent an address — so it is refused as malformed.
    #[test]
    fn a_non_v4_mapped_external_address_is_refused_rather_than_truncated() {
        let nonce = [4u8; 12];
        let mut r = pcp_ok(&nonce, Ipv4Addr::new(81, 2, 69, 142), 50000);
        r[44] = 0x20; // a 2000::/3 global unicast v6 address
        r[54] = 0;
        r[55] = 0;
        assert!(matches!(
            pcp::decode_map_response(&r, &nonce),
            Err(MapError::Malformed(_))
        ));
    }

    // -----------------------------------------------------------------------
    // The shared checks
    // -----------------------------------------------------------------------

    /// `finish` is where a *granted* mapping is still refused, and each of
    /// these is a real router behaviour rather than a hypothetical.
    #[test]
    fn a_granted_mapping_can_still_be_refused_and_each_reason_is_distinct() {
        let ok = finish(Ipv4Addr::new(81, 2, 69, 142), 4041, 50000, 7200, 4041, "PCP")
            .expect("a routable address with a live lease is a mapping");
        assert_eq!(ok.external, SocketAddrV4::new(Ipv4Addr::new(81, 2, 69, 142), 50000));
        assert_eq!(ok.lifetime_secs, 7200);

        // CGNAT: granted, and unreachable.
        assert_eq!(
            finish(Ipv4Addr::new(100, 70, 0, 1), 4041, 50000, 7200, 4041, "PCP"),
            Err(MapError::NotPublishable(Ipv4Addr::new(100, 70, 0, 1)))
        );
        // A zero lifetime is how both RFCs spell DELETED — publishing it would
        // advertise a door the router just told us it closed.
        assert!(matches!(
            finish(Ipv4Addr::new(81, 2, 69, 142), 4041, 50000, 0, 4041, "PCP"),
            Err(MapError::Malformed(_))
        ));
        // An answer about a port we never asked about is not our mapping.
        assert_eq!(
            finish(Ipv4Addr::new(81, 2, 69, 142), 9999, 50000, 7200, 4041, "PCP"),
            Err(MapError::WrongPort { asked: 4041, answered: 9999 })
        );
    }

    /// Renewal is half the **granted** lifetime, not half of what we asked for
    /// — a router that quietly grants less than requested is common, and using
    /// our own number would let those mappings lapse between renewals.
    #[test]
    fn renewal_is_half_the_granted_lease_with_a_floor() {
        let m = |secs| Mapping {
            external: SocketAddrV4::new(Ipv4Addr::new(81, 2, 69, 142), 1),
            lifetime_secs: secs,
            protocol: "PCP",
        };
        assert_eq!(m(3600).renew_after_secs(), 1800);
        assert_eq!(m(120).renew_after_secs(), 60);
        // A gateway granting a 2-second lease must not spin us at 1 Hz.
        assert_eq!(m(2).renew_after_secs(), 30);
    }

    // -----------------------------------------------------------------------
    // The wire, against a fixture that answers like a router
    // -----------------------------------------------------------------------

    /// **The end-to-end shape with no router in the loop.** A fixture socket
    /// speaks PCP on 5351; we drive the real `request_mapping` at it and assert
    /// it comes back with the door the fixture opened. This is what proves the
    /// codecs, the exchange and `finish` compose — each is tested alone above,
    /// and composing them is where a request that never gets sent hides.
    #[tokio::test]
    async fn a_router_that_speaks_pcp_yields_a_mapping() {
        let router = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = router.local_addr().unwrap().port();

        let server = tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let (n, from) = router.recv_from(&mut buf).await.unwrap();
            assert_eq!(n, 60, "a PCP MAP request is 60 bytes");
            assert_eq!(buf[0], 2, "PCP is tried FIRST — a NAT-PMP request here means the order flipped");
            let mut nonce = [0u8; 12];
            nonce.copy_from_slice(&buf[24..36]);
            // Echo the client's own nonce back, as a real server does.
            let mut r = [0u8; 60];
            r[0] = 2;
            r[1] = 1 | 0x80;
            r[4..8].copy_from_slice(&1800u32.to_be_bytes());
            r[24..36].copy_from_slice(&nonce);
            r[36] = 6;
            r[40..42].copy_from_slice(&4041u16.to_be_bytes());
            r[42..44].copy_from_slice(&50000u16.to_be_bytes());
            r[54] = 0xff;
            r[55] = 0xff;
            r[56..60].copy_from_slice(&Ipv4Addr::new(81, 2, 69, 142).octets());
            router.send_to(&r, from).await.unwrap();
        });

        let nonce = [5u8; 12];
        let got = request_mapping_at(
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            Ipv4Addr::new(192, 168, 1, 22),
            4041,
            3600,
            &nonce,
        )
        .await
        .expect("the fixture granted a routable mapping");

        assert_eq!(got.external, SocketAddrV4::new(Ipv4Addr::new(81, 2, 69, 142), 50000));
        assert_eq!(got.protocol, "PCP");
        assert_eq!(got.lifetime_secs, 1800);
        server.await.unwrap();
    }

    /// **The staleness rule, which is the one that makes this a lease.** Only
    /// `Open` yields an address — `Probing` and `Closed` must not let a caller
    /// reach for the last good one. A lapsed mapping published as if it were
    /// live is precisely the "wrong address is worse than none" failure the
    /// transport-profile work already paid for, and the temptation to keep the
    /// last value across a failed renewal is exactly how it comes back.
    #[test]
    fn only_an_open_door_has_an_address() {
        let open = MapState::Open {
            external: SocketAddrV4::new(Ipv4Addr::new(81, 2, 69, 142), 50000),
            protocol: "PCP",
        };
        assert_eq!(
            open.external(),
            Some(SocketAddrV4::new(Ipv4Addr::new(81, 2, 69, 142), 50000))
        );
        assert_eq!(MapState::Probing.external(), None, "not yet is not an address");
        assert_eq!(MapState::Disabled.external(), None);
        assert_eq!(
            MapState::Closed("the router refused".into()).external(),
            None,
            "a failed renewal withdraws the address; it does not keep the last one"
        );
    }

    /// A router that answers nothing is the **expected** case — most consumer
    /// routers speak only UPnP IGD, which this module does not. It must come
    /// back as `NoResponse` in bounded time, not hang and not error.
    #[tokio::test]
    async fn a_silent_router_is_a_bounded_no_not_a_hang() {
        // Nothing is bound here; on loopback that yields ICMP port-unreachable
        // or silence, and both must land as NoResponse.
        let dead = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1);
        let nonce = [6u8; 12];
        let started = std::time::Instant::now();
        let e = request_mapping_at(dead, Ipv4Addr::new(192, 168, 1, 22), 4041, 3600, &nonce).await;
        assert!(
            matches!(e, Err(MapError::NoResponse) | Err(MapError::Io(_))),
            "a silent gateway must be a bounded no, got {e:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the probe must not outlive its retransmit schedule"
        );
    }
}

