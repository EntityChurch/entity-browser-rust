# UI Design Standards — structure & interaction

**The authoritative standard for how a window is *structured* and how it
*behaves*** — grouping, hierarchy, status vocabulary, feedback, and language.
Companion to `REFERENCE-THEMING.md`: **that doc is color** ("it is colors,
nothing more"); **this doc is everything about a screen that isn't color** —
where things sit, what's primary, what a status means, when a view refreshes.

> **Why this exists.** The UI grew window-by-window with no shared structural
> language, so each screen re-invented grouping, button emphasis, and status
> wording. The result reads as incoherent (six unrelated sections in Peer
> Connections; "authorized" said three different ways; upload-then-manually-
> refresh). This doc fixes the *cause*: one set of grounded rules + a few shared
> primitives, so we stop re-deciding per window. It is the **yardstick for
> Phase 3** (`docs/plans/ROADMAP-file-transfer-system-backend.md`).

---

## §0 Stance (what this is, and is not)

- **Not a third-party design system.** We are **not** adopting Material / Bootstrap /
  any kit. Those are *someone else's* answers. We derive from the **durable
  fundamentals those kits are themselves built on** — perceptual grouping,
  visual hierarchy, interaction heuristics — and encode *our* decisions as
  reusable primitives.
- **Fundamentals, then standards, then primitives.** §1 is the grounding (cited).
  §2 is the checkable rules. §3 is the shared code that makes the rules cheap to
  follow. §4 applies them to the two worst offenders.
- **Minimalist ≠ dumbed-down (operator, 2026-07-10).** The goal is **expose the
  right information, well-structured**, so the operator always knows what's going
  on and can act — *not* to hide detail behind sparse screens. Progressive
  disclosure hides **rare/advanced** controls; it never hides the core state.
  When in doubt, show more information but **structure it** (S2 grouping, S7
  tables) rather than removing it.
- **Scope:** the in-app chrome/windows (DOM views under `src/dom/`). Color/token
  mechanics stay in `REFERENCE-THEMING.md`; this doc *references* tokens, never
  redefines them.

---

## §1 The fundamentals we stand on

Four durable ideas, each with the **so-what for us**.

1. **Gestalt — proximity & common region.** Elements placed near each other, or
   inside one shared boundary, are perceived as **one group**; a boundary cue
   *overpowers* mere spacing ([NN/G, Common Region]; [NN/G, Proximity]).
   → *So-what:* a window must read as a small number of clearly-bounded groups,
   each doing one job. Controls that don't belong together must not sit adjacent
   with equal weight. **A control floating between two bounded groups is the
   single most common incoherence** (it's the "what is this doing in the middle"
   feeling).

2. **Visual hierarchy — one primary, de-emphasize the rest** ([Refactoring UI]).
   Every group has **one** primary action (solid, high-contrast); secondary
   actions are outlined/lower-contrast; tertiary read as links. Establish
   hierarchy with **weight and color, not size**; use **2–3 weights max**.
   → *So-what:* we currently render many equal-weight buttons, so nothing is
   "the thing to do." Each group must have exactly one obvious primary.

3. **Nielsen #1 — visibility of system status.** The system always keeps the
   user informed through **timely feedback**; the view must reflect the **true
   current state** ([NN/G, Visibility of System Status]).
   → *So-what:* "upload a file, then go back and hit refresh" is a status-
   visibility failure — the app caused the change and didn't reflect it.

4. **Nielsen #2 & #4 — match the real world, and be consistent.** Speak the
   user's language; **the same concept gets the same word and look everywhere**
   ([Nielsen heuristics]).
   → *So-what:* "✓ authorized" / "pending" / "access unverified" / "not
   authorized" / "denied" are the **same three states** narrated four ways in
   three windows. One vocabulary, one look, one authoritative home.

   *(Corollary — Nielsen #8, minimalist / progressive disclosure: show the
   primary path; put advanced controls behind a disclosure, don't stack them.)*

---

## §2 The standards (checkable rules)

Every window/view must pass these. They're phrased so a review can answer
yes/no.

### S1 — Spacing comes from a scale, not from feel
One constrained scale, base **4px**: **4 · 8 · 12 · 16 · 24**. Nothing else
(today's ad-hoc `6px`/`10px`/`2px` go away). Rule of thumb (Refactoring UI):
**tight within a group (4–8), loose between groups (12–16).** Start generous,
remove until it's just enough — compact is a deliberate choice, never a default.

### S2 — One window = one job; content is 2–4 bounded groups
State the window's job in one line (it already has a title — make the title
honest). Its controls cluster into **named groups**, each in **one bounded
container** (a "card": border/sunken background per common region). **Target
2–4 groups**; more than that means split the window or disclose the extras.
**Every control belongs to exactly one group — nothing floats between them.**

### S3 — One primary action per group
Within a group, **exactly one** primary (`theme::BTN_PRIMARY`). Everything else
is secondary (`BTN_SECONDARY`) or tertiary (`BTN_SMALL`/link). Never two solid
primaries competing in one view. A **disabled** primary must look disabled
(it does), and a **destructive** action (Delete) is never styled as the group's
primary.

### S4 — One status vocabulary, one home, reflected read-only
For each recurring status, there is **one** state model — one word per state,
one color, one glyph — rendered by **one shared helper** (§3), and it lives in
**one authoritative window**; other windows reflect it **read-only** using the
same helper. The two that bite us today:

| Concept | The only states (word · color token · glyph) | Authoritative home |
|---|---|---|
| **Connection** | `Connected` · `--status-ok` · ● / `Connecting…` · `--text-dim` · ◐ / `Offline` · `--text-dim` · ○ | System Backend / Peer Connections |
| **Authorization** | `Authorized` · `--status-ok` · ✓ / `Pending` · `--text-dim` · • / `Not authorized` · `--status-err` · ⛔ | System Backend (the backend owns the grant) |

Banned: "unverified", "access unknown", "known peer" (as a *status*), "denied"
as a separate word — they are just the three states above, said inconsistently.
File Transfer's access chip is a **read-only reflection** of Authorization, not
its own vocabulary.

### S5 — Feedback, the write-refresh rule, and the four states
- **Every action confirms in place** — a result line, a state change, or a
  toast; never silent.
- **Write-refresh rule:** an action that changes a resource **refreshes the view
  of that resource on success**. Upload a file → the share re-lists itself. The
  user never manually refreshes a state the app just caused. (A manual **Refresh**
  stays only for *external* changes we don't observe — e.g. someone else edited
  the backend share.)
- **Design all four states** of any async/list surface, not just the happy one:
  **loading** (a real "Loading…", not a blank), **empty** (a helpful line +
  the next action, not a void), **error** (loud, specific, actionable), and
  **content**.

### S6 — Speak the user's terms
Primary view uses **human names** — device names, "your backend", "this
device" — not `peer_id` hashes or `backend_pid`. Raw ids live behind a
disclosure or in a dim mono field for the people who want them. State values in
context ("Sharing 3 files", not "root_listed: true").

### S7 — Repeated records are tables, not ad-hoc rows
Any surface that lists **the same kind of record more than once** — the peer
roster, device authorizations, known peers, shared-file listings — renders as a
**proper table/columned structure**: aligned columns with a header row naming
each column, one record per row, actions in a trailing column. The current
free-form `flex` rows (name + a couple of buttons crammed together) fail the
test the operator named: *"it's not clear what you're looking at."* Columns make
a list **scannable** — the eye reads down a column (Gestalt: alignment =
continuity) instead of re-parsing each row. Rules:
- **Header row** labels every column; never a bare list of values.
- **Align** by type (text left, numbers/sizes right, status/actions in their own
  column) so rows read as a grid, not a paragraph.
- **Don't dumb it down (S0):** show the columns that matter (id/name, kind,
  status, address, last-seen…) — put only the genuinely-rare detail behind a
  row disclosure. A dense, aligned table beats a sparse one.
- Keep it themed (tokens, `--border` gridlines used sparingly per §1 common-
  region — a zebra/hover background often groups rows better than full borders).

### S8 — A recurring affordance is a shared primitive, not a per-window invention
The rule that fixes the *cause* the operator named: *"stop making up every new
window like we don't have other windows to align with."* Any interaction that
appears in **more than one window** — a collapsible section, a create form, a
status chip, a table — has **one** implementation in `dom/components.rs` that
every window calls. You do not hand-roll a second look for the same job.
- **Before adding an affordance, grep for it.** If Site Creator, Peers, or
  Knowledge Base already does "reveal a create form" / "toggle a section" /
  "list records", you consume that primitive or you promote it — you never add a
  third bespoke variant. (This doc exists *because* we didn't: "New site" was a
  state-driven expander, "Add a peer" was a native `<details>`, "New article" was
  a view-switch — three reveals for one job.)
- **The create-affordance convention.** "Create a new record in this window's
  list" is: a **collapsed** disclosure titled **"New/Add {noun}"**
  (`components::collapsible_header`) → a form **`card`** with **exactly one
  primary** (S3) → **collapses on success** (the model sets the open bool false
  after the create). Open-state is **model-held**, never a native `<details>`:
  a `<details>` re-renders closed on every repaint, so a subscription firing
  mid-entry snaps the form shut under the user. Read fields at submit time (not
  tree-backed), so collapsing loses nothing.

---

## §3 Primitives to add (so following the rules is the easy path)

Reuse-before-abstraction: build these **once**, windows consume them — this is
what makes S1–S6 the *default* instead of per-window discipline.

- **Spacing tokens** (S1) — `SP_1..SP_6` (4/8/12/16/24) in `theme.rs`; replace
  ad-hoc margins as views are touched.
- **`status_chip(state)`** (S4) — the *single* source that renders a Connection
  or Authorization chip (word + color + glyph). Every window calls it; no window
  hand-rolls a status span again.
- **`card(title) -> Element`** (S2) — the bounded group container (the sunken
  card File Transfer already hand-rolls at `file_transfer.rs:67`). Promote it to
  `dom/util` so every group is one call and grouping is automatic.
- **`empty` / `loading` / `error` helpers** (S5) — standard renders for the
  three non-content states, so no view ships a blank.
- **`collapsible_header(ctx, label, open, event) -> Element`** (S8) — the ONE
  disclosure for a create form or a foldable section. The window owns the `open`
  bool (model-held) and dispatches `event` to toggle it. Promoted from Site
  Creator's local copy; Peers + Site Creator both consume it (no more `<details>`
  / bespoke expander split).

These are small and additive; they don't require a redesign to land, and every
Phase-3 move gets cheaper once they exist.

---

## §4 Applying it — the two worst offenders

Diagnosed against §2, with structural **directions to compare** (no visual
mockups — this is about structure, not pixels).

### §4a Peer Connections — "totally incoherent"

**Diagnosis (`dom/peer_connections.rs`):** six stacked sections (Bound info ·
Known Peers · Device Authorizations · Backend Peers · Connect to Address · QR).
- **S2 fail:** no single job — remembering peers, authorizing inbound devices,
  and dialing an address are *three different jobs* in one window.
- **Common-region fail (the unbounded strip in the middle):** "Connect to Address" renders
  as a bare label+input+button **between** bounded groups (`render_manual_connect`,
  no container) — it floats, so it reads as noise.
- **S3 fail:** multiple primaries (Authorize, Connect) compete.
- **S4 fail:** "Known Peers", "Device Authorizations", and connection state each
  speak their own words.

**Direction A — split by direction (inbound vs outbound).** *(aligns with the
existing Phase-3 design.)* **Inbound** ("my backend + who's authorized") moves
into **System Backend**, the window that already *is* the backend's surface.
Peer Connections becomes **one job — "reach another device"**: a single group
[known devices → Reconnect | Connect by address / Scan]. One primary, 1–2
groups, S2/S3/S4 pass. Also retires the redundant Backend Peers section and the
dead overlaps.
*Trade:* two windows to learn, but each is coherent and matches the backend-as-
a-surface we already built.

**Direction B — one window, task-tiered by disclosure.** Keep a single
"Connections" window but restructure top-to-bottom by task: the **primary task**
(connect / reconnect) is the one always-open group; **authorizations, QR, and
advanced** collapse behind labeled disclosures (progressive disclosure, S2
corollary).
*Trade:* fewer windows, but the backend's inbound story stays split from the
System Backend window that owns the grant — weaker S4 "one home".

**Recommendation:** **A.** It gives each window one honest job, fixes the
dead-button and vocabulary overlaps at the same time, and puts authorization
where the grant actually lives.

### §4b File Transfer — "shared files / send a file / upload is a mess"

**Diagnosis (`dom/file_transfer.rs`):** the window presents "Shared files",
"Send a file", and the upload control as if they were unrelated sections.
- **S2:** they're actually **one job** (move files with this device) in **two
  directions** (get / send) — the grouping should say that.
- **S4 dup:** the header shows an access chip *and* `render_access` prints a
  second access paragraph — the same status twice, two looks.
- **S5 fail:** upload/pull don't re-list (the manual-refresh papercut); first
  view needs a "Browse" click.
- **S3:** the disabled Pull primary and the active Upload sit at similar weight.

**Direction — name the job, split by get/send, obey write-refresh.** Title states
the job: **"Files — {device name}"**, with the **one** access chip (S4, read-only
reflection). Body is two bounded groups: **Get** (the tree + one primary "Pull
selected") and **Send** (one primary "Upload / drop a file"). Auto-load the share
on target resolve; **upload re-lists on success** (S5). Delete the redundant
access paragraph — the chip is the single truth.

---

## §5 How this grounds Phase 3

This doc is the **acceptance yardstick** for the Phase-3 consolidation. Nothing
in the roadmap's sequence changes; what changes is that "this window looks wrong" becomes
**measurable** — each window is done when it passes **S1–S6**.

- **Land the §3 primitives first** (`status_chip`, `card`, spacing tokens,
  state helpers) — every later move consumes them, so they pay for themselves.
- **Pull the write-refresh fix (S5) forward** — it's small and it's the daily
  papercut.
- Then 3b–3e execute as designed, each checked against §2. Feed any new friction
  back here (the ratchet).

---

## §6 Deferred / open (operator, 2026-07-10)

Captured so they aren't lost; not this increment's work.

- **Authorization may generalize beyond file-transfer devices** (other grant
  types / peer kinds). When it does, keep it in **one management spot** — do
  **not** spawn a separate authorizations screen per type. Direction A puts
  device-auth in System Backend *for now* precisely because that's the one flow;
  a general "who-can-do-what" surface is a later, deliberate design.
- **"Peers" vs "Peer Connections" is itself confusing** — two windows whose jobs
  blur ("I want to manage my peers in one spot"). Direction A slims Peer
  Connections to outbound, which helps, but the roster/connection overlap is a
  known follow-up to revisit once the split has been driven.

---

## Sources

- [NN/G — The Principle of Common Region: Containers Create Groupings](https://www.nngroup.com/articles/common-region/)
- [NN/G — Proximity Principle in Visual Design](https://www.nngroup.com/articles/gestalt-proximity/)
- [NN/G — Visibility of System Status (Usability Heuristic #1)](https://www.nngroup.com/articles/visibility-system-status/)
- [NN/G — 10 Usability Heuristics for User Interface Design](https://www.nngroup.com/articles/ten-usability-heuristics/)
- [Refactoring UI — book summary (hierarchy, spacing scale, borders vs background, states)](https://www.sglavoie.com/posts/book-summary-refactoring-ui/)
