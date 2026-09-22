# Model — stakeholders, ownership, and the storage partitions

**Ground truth, not a design.** Read this before writing any rule about *who owns a piece of
state*, *what may be refreshed from a remote source*, *what a cutover or rollback may replace*,
or *what a deployment is allowed to decide on a visitor's behalf.

It is a **model**, in the sense the substrate model is: it describes what is already true of the
system, so that designs stop re-deriving it differently each time. Where a design disagrees with
this file, one of them is wrong and it is worth finding out which.

**Why it exists:** four separate design threads — deployment generations, resilience and
reconciliation, code-axis rollback, and capability posture — each independently reached for a
publisher/user ownership line, and each drew it slightly differently. The line they were reaching
for is here, and it has three parties rather than two.

---

## 1. Three roles, and they are positional

| | **Vendor** | **Deployer** | **End user** |
|---|---|---|---|
| Becomes one by | writing the application | putting it on a domain with content | loading a URL |
| **Owns** | the shell, the service worker, the wasm bundles, the worker pair, the loader | the deployment config, published content and tree, the publishing peer identity, CDN configuration, registry pins | their peer identity and keys, their tree, app saves, session posture, anything they authored |
| **Stored in** | HTTP/CDN, and the browser's Cache Storage | HTTP, per domain | **IndexedDB / OPFS**, on their device |
| Changes it by | a release | a publish | using the application |
| Cadence | the vendor's | the deployer's, independent | continuous |
| **Can be reached** | ships code | reads documentation, sometimes | **not at all** |

**The roles are positional, not personal.** One party commonly holds all three — anyone running
the application on their own domain for themselves does. A deployer is always also an end user of
the vendor's product. Reasoning about "the user" without saying *which* role is the most common
way ownership questions come out muddled, and it is why several designs drew the line
differently.

**The last row is the one with consequences.** A vendor can ship a fix. A deployer can be told
something. An end user cannot be reached, cannot be instructed, and cannot be assumed to know
that a cache exists — let alone how to clear one. Any remedy whose final step is *"and then the
visitor does X"* is not a remedy.

---

## 2. Two storage partitions — one exists, one does not

Conflating these produces wrong conclusions in both directions, so they are stated separately.

### 2.1 Partition A — program vs. state. **This exists.**

It is a **substrate** boundary rather than a marking convention, which makes it stronger than a
rule someone has to remember:

| Layer | Substrate | Owner |
|---|---|---|
| The program | HTTP/CDN + the browser's Cache Storage | vendor |
| Deployment policy | HTTP, per domain | deployer |
| Published content and tree | HTTP content store, addressed by hash | deployer |
| Local peer state | **IndexedDB / OPFS** | end user |

Replacing the program means replacing entries in Cache Storage and re-fetching over HTTP. **It
does not write to IndexedDB or OPFS.** So the property that operating-system updaters achieve by
explicitly excluding a data partition from the cutover, this system gets structurally: the paths
never cross.

**What this does *not* protect against.** An older program running against newer local state may
still degrade that state through ordinary use — reading a record it only partly understands and
writing it back without the parts it did not. That is a **codec compatibility** property, not a
partitioning one, and it is not fixed by anything in this section.

### 2.2 Partition B — publisher-seeded vs. user-authored, *within* the end user's tree. **This does not exist.**

Routing facts, catalogs, cached foreign content, session posture, app saves and authored sites
all live in one tree with no ownership marking. Consequences, each observed rather than
predicted:

- **No field can be safely refreshed from its source**, because a value a deployment seeded and a
  value the end user deliberately chose are byte-identical in the entity. The safe-looking move
  is to refresh nothing, and that produces state that is never re-checked.
- **A reset cannot be scoped.** Clearing site data is the only universal repair and it destroys
  everything, including work unrelated to whatever was broken.
- **A generational cutover cannot be partial**, because adopting one would move everything,
  including state the deployer has no business touching.

**This is the gap to close for refresh semantics.** It is not a prerequisite for replacing the
program, and treating it as one over-scopes that work.

---

## 3. The deployer↔end-user line is CONFIGURED, not fixed

This is the part most easily got wrong, and the application already implements it.

A deployment's config carries, today, the controls that decide how much authority an end user
has:

- **`peer_creation_enabled`** — whether the end user may mint a peer at all.
- **`site_mode.locked`, `site_mode.show_toggle`** — whether they can leave the surface they
  landed on.
- **`surface`, `window_type`** — what the application *is* for this audience.
- **`home_site`, `origins`, the registry pin** — who the end user trusts, decided for them.

So deployments span a range. At one end, a **kiosk**: the end user owns almost nothing, the
deployer holds effectively all authority, and there may be no meaningful user peer at all. At the
other, a **full shell**: the end user gets a real peer, real keys, and authors freely.

> **Therefore the publisher/user ownership line is not a constant waiting to be discovered. It is
> a policy the deployer sets, and it has a per-deployment answer.**

Any design that hardcodes that line is wrong at one end of the range. An ownership table is
best read as *the default when a deployment says nothing*, never as the model.

A corollary worth stating because it has been asserted the other way: **"there is no meaningful
user peer" is true only of a deployment where one party holds all three roles.** It is not a
property of the system. A third-party deployer has their own publishing peer and their own end
users, and what those end users own is something that deployment decides.

---

## 4. Who can actually fix what

Classifying a failure by severity says what it costs. It does not say **who can act**, and that
is frequently the more useful fact — a remedy is only real if the party who can perform it is
both reachable and motivated.

| Failure originates in | Harmed | Can remedy | Notes |
|---|---|---|---|
| The program | every end user of **every** deployer | **vendor only** | the widest blast radius available |
| Deployment/CDN configuration | that deployer's end users | **deployer only** | the vendor cannot purge someone else's CDN |
| Published content | that deployer's end users | deployer | republish |
| The end user's own device state | that end user | end user, or nobody | the only case where what is lost is *theirs* |

Three consequences:

1. **A remedy that depends on a third-party deployer noticing is weaker than one that depends on
   the vendor noticing.** The vendor can be assumed to act. A deployer running an unattended
   domain cannot. Designs should treat "the deployer will spot it" as unreliable until detection
   is automatic.
2. **For failures only a deployer can fix, documentation is the vendor's only control surface.**
   That makes deployer-facing guidance a reliability mechanism rather than a courtesy — and makes
   guidance that is wrong, or that contradicts itself between sections, a reliability defect.
3. **Failures that destroy end-user data are categorically different** from failures that break
   the program. The program is re-fetchable by definition; a visitor's authored work is not, and
   there is currently no export of any kind.

---

## 5. How to use this

When writing a rule about state, answer in this order:

1. **Which role owns it?** Vendor, deployer, or end user (§1).
2. **Which substrate is it in?** That usually settles what a replacement can and cannot touch
   (§2.1).
3. **If it is in the end user's tree, is it publisher-seeded or user-authored?** Today the system
   cannot tell — say so rather than assuming (§2.2).
4. **Is the boundary configured?** If the rule concerns deployer vs end user, it varies per
   deployment and the rule must hold across the whole range (§3).
5. **If it fails, who can fix it, and are they reachable?** (§4).

A design that cannot answer 1 and 5 is not ready.
