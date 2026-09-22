//! Window system — multi-instance window manager.
//!
//! Windows are independent instances, each with their own state.
//! The command palette spawns new instances; closing removes them.
//! Multiple windows of the same type can coexist.

use crate::action::Action;
use crate::peers::Peers;

/// Unique identifier for a window instance.
pub type WindowId = u64;

/// Repaint callback — call this to request a frame redraw. Produced only
/// on wasm (the render loop), but the type alias is defined for all
/// targets so app-tier code (e.g. the content-site resolver's late-bound
/// repaint cell) compiles + unit-tests natively.
pub type RepaintFn = std::rc::Rc<dyn Fn()>;

/// Storage for closures that must be kept alive between DOM rebuilds.
/// Cleared at the start of each render cycle, which drops old closures
/// after their DOM elements have been removed.
#[cfg(target_arch = "wasm32")]
pub type ClosureVec = std::rc::Rc<std::cell::RefCell<Vec<wasm_bindgen::JsValue>>>;

/// Create a new empty ClosureVec.
#[cfg(target_arch = "wasm32")]
pub fn new_closure_vec() -> ClosureVec {
    std::rc::Rc::new(std::cell::RefCell::new(Vec::new()))
}

/// What an authoritative durable read of a surface's persisted state actually
/// established.
///
/// **Four outcomes, not a `bool`.** The first draft of this (on the content-site
/// surface, where it was earned) returned "did anything change", which merged
/// *"the tree holds the user's state and we adopted it"* with *"the tree holds
/// nothing, so we moved off the empty placeholder onto the configured
/// default"* — two facts with the same answer, which is the conflation this
/// repo has spent a whole thread removing (`put_if_absent` for *did the user
/// set this*, a presence check for *do I hold current bytes*, `home_is_local`
/// for *may we read the document*). It mattered immediately: the boot log line
/// would have reported an adoption on a profile that had never persisted one.
///
/// Lives here rather than in `views::content_site` because
/// [`WindowView::hydrate_durable`] made it the class's vocabulary — every
/// window that repairs an AP41 read reports in these terms, and a second copy
/// of the enum is how two windows come to mean different things by
/// `NonePersisted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hydration {
    /// The tree held state and this surface adopted it.
    Adopted,
    /// The tree **answered** and holds no state for this surface. The
    /// configured default stands — and nothing is written, because "you have
    /// no history" is not a thing to record.
    NonePersisted,
    /// Nothing was heard — the round-trip failed. **Changed nothing**, which is
    /// the point: a read that cannot answer must never be able to reset a
    /// surface to its default (AP30 corollary (a)).
    Unheard,
    /// A user-driven change landed while the read was in flight and is newer
    /// than anything it could carry, so it was discarded on arrival.
    Superseded,
    /// **Not attempted** — the construction-time synchronous read already
    /// answered, so there was nothing to correct. The normal, healthy Direct-arm
    /// path.
    ///
    /// This used to be [`Superseded`](Self::Superseded) too, and that was AP40
    /// in the reporting layer: *"the read you already had was fine"* and *"you
    /// moved while we were reading"* are different facts about different
    /// situations, and the D13 line printed the same word for both — so an
    /// incident could not tell a healthy boot from one where the guard fired.
    /// Found 2026-08-31 by writing the window-surface gate, whose log assertion
    /// failed on the Direct arm because this case reported **nothing at all**.
    AlreadyResolved,
}

impl Hydration {
    /// The one fact a caller may branch on: did the user's own persisted state
    /// reach the screen?
    pub fn adopted(self) -> bool {
        matches!(self, Hydration::Adopted)
    }

    /// Short label for the D13 boot line.
    pub fn label(self) -> &'static str {
        match self {
            Hydration::Adopted => "adopted",
            Hydration::NonePersisted => "none-persisted",
            Hydration::Unheard => "unheard",
            Hydration::Superseded => "superseded",
            Hydration::AlreadyResolved => "already-resolved",
        }
    }
}

/// What a window did with the address it was opened at — **four facts, and three
/// of them are refusals that must not merge.**
///
/// A spawn carries an optional [`crate::entity_ref::EntityRef`]
/// ([`crate::open_target`]). Most of the 26 roster entries are not viewers of
/// published content and ignore it; the ones that are decode their own payload
/// out of it. The outcomes exist because *"this window does not take targets"*,
/// *"that address is not mine"* and *"it is mine and I cannot act on it"* send a
/// person — and a bug report — to three different places, and a `bool` would
/// render all three as the same shrug (AP40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    /// This window type takes no target. The default, and the honest answer for
    /// every window that is not a viewer of somebody else's published content.
    NotAimable,
    /// Took it — the window opened showing what the caller named.
    Aimed,
    /// This window is aimable and the address is not its convention's. Reachable
    /// only through a caller that routed badly, since
    /// [`crate::open_target::route`] picks the window type *from* the address —
    /// which is exactly why it is worth a distinct word rather than a silent
    /// no-op.
    NotMine,
    /// The address is this viewer's and it still cannot be opened at it — the
    /// Registry Browser's *"this publisher's sites"* with no site named is the
    /// live instance. **Not a failure**: the window opens as it otherwise would,
    /// and this records that the aim added nothing.
    Unusable(&'static str),
}

impl Aim {
    /// Short label for the D13 line. Five words for four outcomes is a bug;
    /// `every_aim_outcome_has_its_own_word` asserts the count.
    pub fn label(self) -> &'static str {
        match self {
            Aim::NotAimable => "not-aimable",
            Aim::Aimed => "aimed",
            Aim::NotMine => "not-mine",
            Aim::Unusable(_) => "unusable",
        }
    }

    /// The one fact a caller may branch on: is the window showing what was asked
    /// for?
    pub fn hit(self) -> bool {
        matches!(self, Aim::Aimed)
    }
}

/// How a window is named where several are listed: its title, and the app it
/// runs when it runs one — *Apps · Alpine Linux*.
pub fn display_title(view: &dyn WindowView) -> String {
    match view.running_app() {
        Some(app) if !app.trim().is_empty() => {
            crate::i18n::t("palette.running_title", &[("window", &view.title()), ("app", &app)])
        }
        _ => view.title(),
    }
}

/// A window view that renders into web-sys DOM.
#[allow(dead_code)]
pub trait WindowView {
    /// Display title (may include instance context, e.g., current path).
    fn title(&self) -> String;

    /// Type name for the command palette (e.g., "Entity Browser").
    fn type_name(&self) -> &'static str;

    /// The peer this window is bound to.
    ///
    /// ⭐⭐ **UNDEFAULTED, AND THAT IS THE ENFORCEMENT POINT (2026-09-18).**
    ///
    /// It defaulted to `""` — so a window that simply did not implement it was
    /// bound to a peer, read that peer's tree on every render, and **reported no
    /// binding to the window manager**, which is the only thing that matters
    /// here: `find_open` matches on `(type_name, peer_id)`, so `"" == pid` is
    /// false for every real peer and such a window **can never be found**.
    ///
    /// Three windows relied on the default and two of them held a `peer_id`
    /// field they used everywhere internally. The consequences, none of which
    /// raises an error anywhere:
    ///
    /// - **Every aimed open stacks another window.** *Open in Feed* pressed
    ///   four times left four Feed windows, each showing a publisher the person
    ///   had moved on from — which is how this was found, by a gate written for
    ///   the reuse rule that reds against a window the reuse rule cannot see.
    /// - **The singleton-windows setting silently did not apply to them.**
    /// - **`CloseWindow` removed window state at a path built from `""`** —
    ///   inert for these three (none persists any) and not a property to rely on.
    ///
    /// ⇒ ***a defaulted accessor for a fact the caller cannot do without is a
    /// rule with no enforcement point*** — `publish_axes::carried_peers`' own
    /// lesson, which chose `error[E0046]` for exactly this reason and was
    /// written three days after this default was last read. A window with no
    /// peer of its own answers with the one it was constructed against; there is
    /// no honest `""`.
    fn peer_id(&self) -> &str;

    /// The **remote** peers this window keeps a standing relationship with — a
    /// conversation the user deliberately bound and expects to survive a
    /// connection drop. Empty (the default) for every window with no remote
    /// binding.
    ///
    /// A list, not one peer: a conversation's roster (§6) can hold more than two
    /// participants, and maintaining only the first would silently leave the
    /// rest of a group to drop.
    ///
    /// Drives `maintain-peer`: `EntityApp::sync_maintained_peers` sweeps the
    /// open windows and hands each pair to the EXTENSION-NETWORK driver, which
    /// then owns connect-on-drop for it — the same treatment the system backend
    /// gets. Return only targets the user *chose*; a target derived from
    /// "whoever is currently connected" is circular here (it is a target
    /// *because* it is connected) and must stay out.
    fn maintained_remotes(&self) -> Vec<String> { Vec::new() }

    /// The app running inside this window, by name, while one is mounted.
    /// `None` (the default) for every window that hosts no app frame. Two Apps
    /// windows running different apps were indistinguishable (field report
    /// 2026-09-14), so the header shows it beside the title, the palette's
    /// open-window list and the System Monitor name the window by it, and the
    /// monitor knows which windows run code it cannot see into.
    ///
    /// **Beside the title, not in it**: [`Self::title`] stays the window type's
    /// name, which is what every gate and probe locates a window by.
    fn running_app(&self) -> Option<String> { None }

    /// The running app's stable identity (`{set}/{id}`), for keying things a
    /// person sets per app rather than per window type — today, the window's
    /// remembered height ([`crate::window_size`]). `None` whenever
    /// [`Self::running_app`] is. An id rather than the display name, which a
    /// republished catalog may change.
    fn running_app_key(&self) -> Option<String> { None }

    /// Subscription-driven dirty flag for this window. The DOM renderer
    /// uses [`WindowWatch::take_dirty`] to decide whether to rebuild
    /// the section: dirty → rebuild and clear; clean → skip entirely
    /// (preserves DOM-side state like input contents and scroll
    /// position).
    ///
    /// Implementations subscribe their watch to the tree paths their
    /// render reads — see `views/*/mod.rs` for examples.
    fn watch(&self) -> &crate::window_watch::WindowWatch;

    /// Handle an action targeted at this window.
    /// Peers provides tree access for entity-backed state.
    fn handle_action(&mut self, action: &Action, peers: &Peers);

    /// Adopt this window's persisted state from the **durable tree**,
    /// correcting the best-effort synchronous read its factory did at
    /// construction. Default: no-op.
    ///
    /// # Why every window needs this offered to it — AP41
    ///
    /// Every window factory here calls `model.initialize(peers)` before it
    /// subscribes, and `initialize` reads with `Peers::get_entity`, which
    /// answers from the **per-prefix cache mirror**. On the Worker arm that
    /// mirror is never primed for a path nobody has subscribed yet, so the read
    /// returns `None` on a warm boot with perfectly good persisted state; on
    /// the shipped Direct-IDB arm it *races* the store filling from IndexedDB
    /// (measured as a 1-in-3 flake). **Reordering does not fix it** — `observe`
    /// is async, so a subscription makes the *next* read work and a constructor
    /// that caches has no next read. The bug is the retention, not the read.
    ///
    /// # Why the call site is [`WindowManager::spawn`] and not each factory
    ///
    /// The content-site repair originally called its own
    /// `spawn_hydrate_durable` from inside its factory, which made *"windows
    /// get a hydrate step"* something the next window author had to remember.
    /// Calling it from `spawn` makes it structural, and gives AP41 the
    /// class-level enforcement point it previously lacked (it had one only for
    /// `content_site`).
    ///
    /// # What an override owes, and what it must not share
    ///
    /// Fire-and-forget: this is sync because a window is constructed long after
    /// `boot_load` has finished awaiting things, so an override spawns its own
    /// round-trip and adopts through its `Arc<Mutex<_>>`. Two traps that do
    /// **not** generalise, which is why this is a hook and not a helper:
    ///
    /// * **An errored round-trip is not an answer** — keep what you have
    ///   ([`Hydration::Unheard`]). A cache that drops what it cannot re-verify
    ///   turns a hiccup into a lost session (AP30 corollary (a)).
    /// * **A change that lands during the await is newer than it** — guard, or
    ///   you drag the user backwards ([`Hydration::Superseded`]). A shared
    ///   helper that skipped this would reintroduce the user-themes
    ///   resurrection race in every window at once.
    ///
    /// And one that is specific to any surface holding session-only display
    /// state: **merge, do not replace.** The Shell's `scrollback` is
    /// deliberately not persisted, so assigning a decoded state over the live
    /// one would wipe what is on screen — AP41 pointed the other way.
    fn hydrate_durable(&self, _peers: &Peers) {}

    /// Open this window **at an address** — the generic *"open this thing"* that
    /// `Action::SpawnWindow` had no field for. Default: this window type takes no
    /// target.
    ///
    /// # The subject is not the binding
    ///
    /// `target.peer()` is *whose content this is*; `peer_id()` is *whose store we
    /// read*. They are usually different and conflating them is a shipped bug —
    /// see `views/registry_browser/output.rs:open_target`, where binding a Site
    /// Browser to the publisher produced a real window with an empty rail because
    /// no local SDK hosts that id. An override decodes the subject out of the
    /// target and leaves its own binding alone.
    ///
    /// # Why the call site is [`WindowManager::spawn`], AFTER `hydrate_durable`
    ///
    /// Same structural reason as [`hydrate_durable`](Self::hydrate_durable) — a
    /// step every window is offered must not be something a factory author has to
    /// remember (AP44). The **ordering** is load-bearing and is not a new guard:
    /// `durable_hydration_job` takes its witness *synchronously*, inside the
    /// `hydrate_durable` call, so an aim applied after it makes the in-flight read
    /// land on [`Hydration::Superseded`] and keep the aim. An explicit address is
    /// newer than a persisted one — that is what asking for it means — and the
    /// existing witness already says so.
    ///
    /// # What an override owes
    ///
    /// Decode `open_target::payload(target, your_segment)` rather than re-parsing
    /// the path: the outer address is `open_target`'s and the payload is yours.
    /// Answer [`Aim::NotMine`] for an address outside your segment and
    /// [`Aim::Unusable`] for one inside it that names nothing you can open —
    /// never `Aimed` for a no-op, or the D13 line reports a window showing
    /// something it is not.
    fn aim(&mut self, _target: &crate::entity_ref::EntityRef, _peers: &Peers) -> Aim {
        Aim::NotAimable
    }

    /// Per-frame tick, called on every rAF frame (not gated by the dirty
    /// flag, unlike [`render_dom`](Self::render_dom)). Default: no-op. Windows
    /// that must make progress every frame regardless of a tree change —
    /// draining a delivery queue, pumping an async pipeline — override this.
    /// Keep it cheap: it runs ~60×/s. A window that changes tree state here
    /// (e.g. caching a delivered entity) wakes its own render through the
    /// normal subscription path, so no manual dirty-marking is needed.
    fn tick(&mut self, _peers: &Peers) {}

    /// Render into a DOM container. This is THE rendering method.
    /// Every window implements this — build DOM elements, attach event
    /// handlers, set innerHTML, whatever the window needs.
    ///
    /// Use `ctx` helpers for event handlers:
    /// - `ctx.on_window_event(el, "click", "event_name", "value")` — static WindowEvent
    /// - `ctx.on_select_change(el, "event_name")` — <select> change → WindowEvent
    /// - `ctx.on_action(el, "click", Action::Foo)` — push any Action
    /// - `ctx.listen(el, "click", |e| { ... })` — custom handler
    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        state: &Peers,
        ctx: &crate::dom::util::DomCtx,
    );
}

/// User-facing menu grouping for a window type. **Orthogonal to
/// [`WindowScope`]**: scope decides which *peer* a window binds to (an internal
/// concern), category decides which *menu group* it appears under (what the user
/// sees). A category can mix scopes — e.g. `System` holds both the system-scoped
/// Settings and the peer-scoped Peer Connections.
///
/// The roster→category mapping lives in one place ([`crate::window_registry`]),
/// so re-sorting groups is a single-file edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCategory {
    /// Everyday surfaces — what people actually open the app for. Expanded on
    /// first paint. Apps, Entity Native Apps, Chat, Site Browser, Site Creator,
    /// Knowledge Base.
    AppsContent,
    /// The "control panel" — manage your peers, identity, storage, settings.
    /// User-facing but occasional; collapsed by default.
    System,
    /// Power/inspection tools (consoles, shell, taps, raw tree). Collapsed by
    /// default; not hidden — developers can find them.
    Developer,
}

impl WindowCategory {
    /// Whether this group's disclosure starts open. Only the everyday group is
    /// expanded on first paint, so a fresh user lands on Apps / Sites.
    pub fn open_by_default(self) -> bool {
        matches!(self, WindowCategory::AppsContent)
    }

    /// Stable key used to persist this group's open/closed toggle across rebuilds.
    /// Also the i18n key stem ([`label_i18n`]/[`description_i18n`]) — a durable
    /// identity, unlike the translated [`label`]/[`description`] display strings.
    pub fn key(self) -> &'static str {
        match self {
            WindowCategory::AppsContent => "apps",
            WindowCategory::System => "system",
            WindowCategory::Developer => "developer",
        }
    }

    /// Localized section heading (the display form of [`label`]; `label` stays
    /// the English canonical). Keyed `category.<key>`.
    pub fn label_i18n(self) -> String {
        crate::i18n::t(&format!("category.{}", self.key()), &[])
    }

    /// Localized [`description`]. Keyed `category.<key>.desc`.
    pub fn description_i18n(self) -> String {
        crate::i18n::t(&format!("category.{}.desc", self.key()), &[])
    }

    /// All categories, in display order.
    pub fn all() -> [WindowCategory; 3] {
        [
            WindowCategory::AppsContent,
            WindowCategory::System,
            WindowCategory::Developer,
        ]
    }
}

/// Whether a window type is infrastructure or peer-scoped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowScope {
    /// Infrastructure window — always binds to the system peer.
    /// Event Log, Peers, Peer Connections, Key Manager, Settings.
    System,
    /// Peer-scoped window — binds to the user-selected peer.
    /// Entity Tree, Execute Console, Query Console.
    Peer,
}

/// Factory info for spawning windows from the command palette.
///
/// `Clone` is cheap — every field is `Copy` (`&'static str`, an enum, a fn
/// pointer) — which lets a single source ([`crate::window_registry`]) build the
/// list once and hand clones to both the registrar and the settings UI.
#[derive(Clone)]
pub struct WindowType {
    pub name: &'static str,
    #[allow(dead_code)]
    pub description: &'static str,
    /// System vs Peer scope. Read by the command palette (System-scoped spawn
    /// buttons bind the system peer; Peer-scoped bind the selected peer) and by
    /// the startup-surface settings control, which filters the window-type
    /// dropdown by scope against the chosen peer (non-system peers see only
    /// Peer-scoped types — handoff §4.4).
    pub scope: WindowScope,
    /// Factory: receives (window_id, peer_id, peer_manager).
    pub create: fn(WindowId, &str, &Peers) -> Box<dyn WindowView>,
}

/// Human display label for a window-type key. Menus, pickers, and taskbars show
/// THIS — now the **localized** title (`i18n::window_title`), keyed off the
/// canonical name (`window.<slug>`), which is exactly the "friendlier label in
/// one place" this indirection was built for. The canonical `name` stays the
/// identity key. Resolves through the SAME `i18n::window_title` as the matching
/// `WindowView::title()`, so the two are in sync by construction.
pub fn window_display_name(name: &str) -> String {
    crate::i18n::window_title(name)
}

/// Resolve a possibly-legacy window-type key to its current canonical key. A
/// renamed type keeps its old key working here so stale **persisted references**
/// (a boot-surface `window_type` saved before the rename, a build-time
/// deployment config) still spawn. Add each rename as an arm; new callers use the
/// canonical key directly.
pub fn canonical_window_type(name: &str) -> &str {
    match name {
        // Renamed 2026-07-13: the System governance window (see
        // TERMINOLOGY-AND-WINDOWS.md).
        "System Backend" => "System Overview", // i18n-ignore — legacy→canonical type keys, not UI
        // Merged 2026-08-19: Games and Apps are one launcher with category
        // filters. A persisted workspace, a baked `ENTITY_STARTUP_WINDOW_TYPE`,
        // or a shell verb naming "Games" must still open something — without
        // this it resolves to no factory and silently opens nothing.
        "Games" => "Apps", // i18n-ignore — legacy→canonical type keys, not UI
        other => other,
    }
}

impl WindowType {
    /// Display label for menus/pickers (see [`window_display_name`]) — the
    /// localized title; the canonical `name` stays the identity key.
    pub fn display_name(&self) -> String {
        window_display_name(self.name)
    }
}

/// A living window instance.
pub struct WindowInstance {
    pub id: WindowId,
    pub open: bool,
    pub view: Box<dyn WindowView>,
}

/// Manages all window instances and available types.
pub struct WindowManager {
    pub windows: Vec<WindowInstance>,
    next_id: WindowId,
    pub types: Vec<WindowType>,
    /// `(type_name, peer_id)` → the id that pair held last session, from the
    /// durable window index. A pair present here re-opens onto **its own** id
    /// instead of the next ordinal, which is what makes
    /// `window_state_path` land on this window's own state regardless of the
    /// order the user re-opens things in. Consumed as claims are taken — an
    /// entry is good for one window.
    ///
    /// Empty is the correct cold state and the correct *failed-read* state:
    /// no claims means "allocate fresh", which is exactly the pre-index
    /// behaviour. See [`crate::window_index`].
    claims: std::collections::HashMap<(String, String), WindowId>,
    /// Previous-session entries whose window has **not** been re-opened yet.
    ///
    /// These are re-emitted into the durable index alongside the live windows,
    /// and that is not bookkeeping — it is the difference between the feature
    /// working and it deleting people's state. The index is rewritten from the
    /// live set, and a boot where the user opens nothing has an empty live set;
    /// without this, that boot would persist an empty index and the *next*
    /// boot's sweep would find every slot unreachable and delete it. A window
    /// nobody re-opened is not a window that was closed.
    ///
    /// An entry leaves here exactly when its window re-opens (it becomes live)
    /// or when that window is explicitly closed (which already deletes its
    /// state). So the set shrinks on use and never on absence.
    retained: Vec<crate::window_index::WindowIndexEntry>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            types: Vec::new(),
            claims: std::collections::HashMap::new(),
            retained: Vec::new(),
        }
    }

    /// Install the previous session's window index: its claims, and the floor
    /// its ids put under fresh allocation.
    ///
    /// **The floor is raised, never lowered.** `adopt_index` can be reached
    /// after windows already exist (a second peer's index, a late read), and
    /// dropping `next_id` back would hand out an id a live window is holding.
    pub fn adopt_index(&mut self, index: &crate::window_index::WindowIndex) {
        self.claims.extend(index.claims());
        self.retained.extend(index.entries.iter().cloned());
        self.next_id = self.next_id.max(index.next_id_floor());
    }

    /// The id the next unclaimed window will take. Reported on the boot line,
    /// and the thing the index's whole floor argument is about.
    pub fn peek_next_id(&self) -> WindowId {
        self.next_id
    }

    /// Raise the fresh-id floor without installing any claims — the
    /// `NoIndex` / `Malformed` / `Unheard` path.
    ///
    /// A boot that could not read an index still knows which slots are
    /// *occupied* (it can list them), and allocating above them is strictly
    /// safer than starting at 1: it is what keeps a first-boot-after-upgrade
    /// profile from opening a window straight onto pre-index state that no
    /// claim can vouch for.
    pub fn raise_id_floor(&mut self, floor: WindowId) {
        self.next_id = self.next_id.max(floor);
    }

    /// The set of window slots that hold state worth keeping: the open windows,
    /// plus every previous-session slot nobody has re-opened yet.
    ///
    /// This is the **witness** the persist path compares against, per AP44:
    /// nothing has to remember to announce a spawn or a close, because the
    /// answer is derived from `self.windows` every time it is asked. A future
    /// code path that creates or destroys a window is covered the day it is
    /// added, which a notification-based trigger would not be — the Shell's
    /// generation counter read zero in the very test written to exercise it.
    pub fn durable_index(&self) -> crate::window_index::WindowIndex {
        let mut entries: Vec<crate::window_index::WindowIndexEntry> = self
            .windows
            .iter()
            .filter(|w| w.open)
            .map(|w| crate::window_index::WindowIndexEntry {
                id: w.id,
                type_name: w.view.type_name().to_string(),
                peer_id: w.view.peer_id().to_string(),
            })
            .collect();
        entries.extend(self.retained.iter().cloned());
        // Stable order so a byte-comparison witness does not see a change that
        // is only a reordering — the whole persist path is "did these bytes
        // move", and `windows` is a Vec whose order is spawn order.
        entries.sort_by_key(|e| e.id);
        crate::window_index::WindowIndex { entries }
    }

    /// Drop any retained (not-yet-re-opened) index entry for this id.
    ///
    /// Called when a window is explicitly closed, which is also when its
    /// per-window state is deleted — the two have to move together or the index
    /// would keep advertising a slot whose state is gone.
    pub fn forget_retained(&mut self, id: WindowId) {
        self.retained.retain(|e| e.id != id);
    }

    /// Register a window type that can be spawned from the command palette.
    pub fn register_type(&mut self, window_type: WindowType) {
        self.types.push(window_type);
    }

    /// Spawn a new window instance of the given type bound to a peer, with no
    /// address — the ordinary open. Returns its ID.
    ///
    /// Delegates to [`spawn_at`](Self::spawn_at) so there is exactly one place a
    /// window is constructed and therefore exactly one place the hydrate and aim
    /// steps are offered.
    pub fn spawn(&mut self, type_name: &str, peer_id: &str, peers: &Peers) -> Option<WindowId> {
        self.spawn_at(type_name, peer_id, None, peers)
    }

    /// Spawn a window **at an address** ([`crate::open_target`]).
    ///
    /// `peer_id` is the store the window reads; `target` names whose content it
    /// is looking at. Those are two facts and they are usually different — see
    /// [`WindowView::aim`].
    pub fn spawn_at(
        &mut self,
        type_name: &str,
        peer_id: &str,
        target: Option<&crate::entity_ref::EntityRef>,
        peers: &Peers,
    ) -> Option<WindowId> {
        // Resolve legacy keys (e.g. a boot-surface saved before a type rename).
        let type_name = canonical_window_type(type_name);
        let factory = self.types.iter().find(|t| t.name == type_name)?;
        // Claim this `(type, peer)` pair's id from the previous session if it
        // has one, so the factory's `window_state_path` addresses this window's
        // OWN state. Without it the id is a bare ordinal and which state a
        // window lands on is decided by re-open order (see `window_index`).
        let id = match self.claims.remove(&(type_name.to_string(), peer_id.to_string())) {
            Some(claimed) => {
                tracing::info!(
                    window_type = %type_name,
                    peer_id = %peer_id,
                    window_id = claimed,
                    "window claimed its previous session's slot"
                );
                // It is live now, so it is no longer a retained slot — leaving
                // it would double-list the id in the durable index.
                self.retained.retain(|e| e.id != claimed);
                claimed
            }
            None => {
                let fresh = self.next_id;
                self.next_id += 1;
                fresh
            }
        };
        let mut view = (factory.create)(id, peer_id, peers);
        // Correct the factory's synchronous `initialize` read with an
        // authoritative one (AP41). Structural rather than per-factory on
        // purpose: this is the class's enforcement point, so a new window type
        // that retains a sync read is covered by overriding one trait method
        // instead of by remembering to add a call here. Default is a no-op, so
        // this costs nothing for the windows that persist nothing.
        view.hydrate_durable(peers);
        // …and THEN the aim, in that order. `durable_hydration_job` takes its
        // witness synchronously inside the call above, so a target applied here
        // makes the in-flight read report `Superseded` and keep what the caller
        // asked for. An explicit address is newer than a persisted one; the
        // existing witness already expresses that and no second guard is owed.
        if let Some(target) = target {
            let outcome = view.aim(target, peers);
            // Reported for every aimed spawn, including the refusals: a window
            // opened at an address and a window that merely opened are the same
            // pixels, and an aim that quietly did nothing is exactly the failure
            // this whole seam exists to remove.
            tracing::info!(
                window_type = %type_name,
                window_id = id,
                bound_peer = %peer_id,
                subject_peer = %target.peer(),
                target = %crate::open_target::write(Some(target)),
                outcome = outcome.label(),
                detail = match outcome {
                    Aim::Unusable(d) => d,
                    _ => "",
                },
                "window opened at a target"
            );
        }
        self.windows.push(WindowInstance {
            id,
            open: true,
            view,
        });
        Some(id)
    }

    /// Find an open window of the given type bound to the given peer.
    /// Used by single-instance ("singleton") window mode to focus an
    /// existing window instead of spawning a duplicate. Identity is the
    /// `(type_name, peer_id)` pair — a window type opened for two
    /// different peers is two distinct windows.
    pub fn find_open(&self, type_name: &str, peer_id: &str) -> Option<WindowId> {
        let type_name = canonical_window_type(type_name);
        self.windows
            .iter()
            .find(|w| w.open && w.view.type_name() == type_name && w.view.peer_id() == peer_id)
            .map(|w| w.id)
    }

    /// Close a specific window instance.
    pub fn close(&mut self, id: WindowId) {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.open = false;
        }
    }

    /// Remove all closed windows.
    pub fn gc_closed(&mut self) {
        self.windows.retain(|w| w.open);
    }

    /// Get a window by ID.
    #[allow(dead_code)]
    pub fn get(&self, id: WindowId) -> Option<&WindowInstance> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Get a mutable window by ID.
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowInstance> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Number of open windows.
    pub fn open_count(&self) -> usize {
        self.windows.iter().filter(|w| w.open).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyView {
        name: String,
        watch: crate::window_watch::WindowWatch,
    }

    impl WindowView for DummyView {
        fn title(&self) -> String {
            self.name.clone()
        }
        fn type_name(&self) -> &'static str {
            "Dummy"
        }
        fn peer_id(&self) -> &str {
            ""
        }
        fn watch(&self) -> &crate::window_watch::WindowWatch {
            &self.watch
        }
        fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}
    }

    fn dummy_type() -> WindowType {
        WindowType {
            name: "Dummy",
            description: "Test window",
            scope: WindowScope::System,
            create: |_id, _peer_id, _pm| Box::new(DummyView {
                name: "Dummy".into(),
                watch: crate::window_watch::WindowWatch::new(),
            }),
        }
    }

    fn test_peers() -> Peers {
        Peers::new_direct()
    }

    // -- Window-index fixtures ------------------------------------------------
    //
    // Two *distinct* window types that both persist state and both report a
    // bound peer — the configuration the index exists for. `DummyView` above
    // cannot serve: its `type_name` is a fixed literal and it never overrides
    // `peer_id`, so every window it makes is the same `(type, peer)` pair and
    // claiming would be untestable against it.

    struct TypedDummy {
        type_name: &'static str,
        peer_id: String,
        watch: crate::window_watch::WindowWatch,
    }

    impl WindowView for TypedDummy {
        fn title(&self) -> String {
            self.type_name.to_string()
        }
        fn type_name(&self) -> &'static str {
            self.type_name
        }
        fn peer_id(&self) -> &str {
            &self.peer_id
        }
        fn watch(&self) -> &crate::window_watch::WindowWatch {
            &self.watch
        }
        fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}
    }

    fn make_alpha(_id: WindowId, peer_id: &str, _p: &Peers) -> Box<dyn WindowView> {
        Box::new(TypedDummy {
            type_name: "Alpha",
            peer_id: peer_id.to_string(),
            watch: crate::window_watch::WindowWatch::new(),
        })
    }

    fn make_beta(_id: WindowId, peer_id: &str, _p: &Peers) -> Box<dyn WindowView> {
        Box::new(TypedDummy {
            type_name: "Beta",
            peer_id: peer_id.to_string(),
            watch: crate::window_watch::WindowWatch::new(),
        })
    }

    fn indexed_manager() -> WindowManager {
        let mut mgr = WindowManager::new();
        mgr.register_type(WindowType {
            name: "Alpha",
            description: "Test window A",
            scope: WindowScope::System,
            create: make_alpha,
        });
        mgr.register_type(WindowType {
            name: "Beta",
            description: "Test window B",
            scope: WindowScope::System,
            create: make_beta,
        });
        mgr
    }

    /// **The defect, and the falsifier for the claim the whole feature rests
    /// on.** Without an index, which persisted state a re-opened window lands
    /// on is decided by the order the user re-opens windows in — nothing about
    /// the window itself.
    ///
    /// Last session: Alpha=1, Beta=2. This session the user opens **Beta
    /// first**. It draws ordinal 1 and addresses Alpha's slot; Beta's own state,
    /// at 2, is unreachable for the rest of the session.
    ///
    /// This is the control. It asserts today-without-the-index behaviour on
    /// purpose, so the test below is measuring a repair rather than restating
    /// an implementation.
    #[test]
    fn without_an_index_a_reopened_window_lands_on_whichever_slot_its_ordinal_draws() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let mut mgr = indexed_manager();

        let beta = mgr.spawn("Beta", &pid, &peers).expect("spawn");
        assert_eq!(
            beta, 1,
            "a fresh manager hands out ordinal 1 regardless of which type asks"
        );
        assert_eq!(
            crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, beta),
            crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1),
            "Beta is addressing the slot Alpha wrote last session"
        );
    }

    /// **The repair.** With the previous session's index adopted, Beta re-opens
    /// onto Beta's slot no matter what order it is opened in — and Alpha, opened
    /// second, still gets its own.
    ///
    /// Both orders are asserted in one test on purpose: the property is
    /// *order-independence*, and checking one order would pass for a fix that
    /// merely permuted the bug.
    #[test]
    fn an_adopted_index_gives_each_window_its_own_slot_in_either_reopen_order() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let prior = crate::window_index::WindowIndex {
            entries: vec![
                crate::window_index::WindowIndexEntry {
                    id: 1,
                    type_name: "Alpha".into(),
                    peer_id: pid.clone(),
                },
                crate::window_index::WindowIndexEntry {
                    id: 2,
                    type_name: "Beta".into(),
                    peer_id: pid.clone(),
                },
            ],
        };

        // Beta first — the order that breaks without the index.
        let mut mgr = indexed_manager();
        mgr.adopt_index(&prior);
        assert_eq!(mgr.spawn("Beta", &pid, &peers), Some(2));
        assert_eq!(mgr.spawn("Alpha", &pid, &peers), Some(1));

        // Alpha first — the order that happened to work anyway.
        let mut mgr = indexed_manager();
        mgr.adopt_index(&prior);
        assert_eq!(mgr.spawn("Alpha", &pid, &peers), Some(1));
        assert_eq!(mgr.spawn("Beta", &pid, &peers), Some(2));
    }

    /// A claim is good for **one** window. A second window of the same
    /// `(type, peer)` pair gets a fresh id above every id the index knew — it
    /// must not be handed the first one's state, and it must not be handed a
    /// slot some other retained entry is sitting on.
    #[test]
    fn a_second_window_of_a_claimed_pair_gets_a_fresh_id_above_the_index() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let mut mgr = indexed_manager();
        mgr.adopt_index(&crate::window_index::WindowIndex {
            entries: vec![
                crate::window_index::WindowIndexEntry {
                    id: 1,
                    type_name: "Alpha".into(),
                    peer_id: pid.clone(),
                },
                crate::window_index::WindowIndexEntry {
                    id: 6,
                    type_name: "Beta".into(),
                    peer_id: pid.clone(),
                },
            ],
        });
        assert_eq!(mgr.spawn("Alpha", &pid, &peers), Some(1), "claims its slot");
        assert_eq!(
            mgr.spawn("Alpha", &pid, &peers),
            Some(7),
            "the second Alpha allocates above id 6, not onto it"
        );
    }

    /// Same type, **different peer**, is a different window. The claim key is
    /// the pair, and a peer-scoped window opened for peer B must not take the
    /// slot peer A's window of the same type left behind — their state lives in
    /// different trees, so adopting it would be a cross-peer read.
    ///
    /// **Both halves are asserted in one manager on purpose.** The refusal
    /// alone is satisfied by the id floor, so on its own it passes with claiming
    /// deleted entirely — measured, and exactly the "green by fallback" shape
    /// this repo has already shipped once. Claiming the *matching* pair right
    /// after is what makes the refusal mean something.
    #[test]
    fn a_claim_does_not_cross_peers() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let mut mgr = indexed_manager();
        mgr.adopt_index(&crate::window_index::WindowIndex {
            entries: vec![
                crate::window_index::WindowIndexEntry {
                    id: 3,
                    type_name: "Alpha".into(),
                    peer_id: "SOME-OTHER-PEER".into(),
                },
                crate::window_index::WindowIndexEntry {
                    id: 5,
                    type_name: "Beta".into(),
                    peer_id: pid.clone(),
                },
            ],
        });
        assert_eq!(
            mgr.spawn("Alpha", &pid, &peers),
            Some(6),
            "another peer's Alpha slot is not ours to claim; allocate above the index"
        );
        assert_eq!(
            mgr.spawn("Beta", &pid, &peers),
            Some(5),
            "and a pair that DOES match still claims — without this the assertion \
             above passes with claiming removed altogether"
        );
    }

    /// **The data-loss guard.** A boot where the user re-opens nothing must not
    /// persist an empty index — the next boot's sweep would read that as "every
    /// slot is unreachable" and delete state nobody closed.
    ///
    /// A window nobody re-opened is not a window that was closed.
    #[test]
    fn an_unopened_prior_window_stays_in_the_index() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let mut mgr = indexed_manager();
        let prior = crate::window_index::WindowIndex {
            entries: vec![crate::window_index::WindowIndexEntry {
                id: 1,
                type_name: "Alpha".into(),
                peer_id: pid.clone(),
            }],
        };
        mgr.adopt_index(&prior);

        assert_eq!(
            mgr.durable_index(),
            prior,
            "with no window re-opened, the index must still name the slot"
        );

        // Re-opening it moves it from retained to live — the same entry, not a
        // second one.
        mgr.spawn("Alpha", &pid, &peers).expect("spawn");
        assert_eq!(
            mgr.durable_index(),
            prior,
            "a claimed slot is listed once, as a live window"
        );
    }

    /// Closing a window that was never re-opened this session drops its slot.
    /// The `CloseWindow` path deletes the state; the index has to stop
    /// advertising it in the same breath or it points at nothing.
    #[test]
    fn forgetting_a_retained_slot_removes_it_from_the_index() {
        let pid = "PEER1".to_string();
        let mut mgr = indexed_manager();
        mgr.adopt_index(&crate::window_index::WindowIndex {
            entries: vec![
                crate::window_index::WindowIndexEntry {
                    id: 1,
                    type_name: "Alpha".into(),
                    peer_id: pid.clone(),
                },
                crate::window_index::WindowIndexEntry {
                    id: 2,
                    type_name: "Beta".into(),
                    peer_id: pid.clone(),
                },
            ],
        });
        mgr.forget_retained(1);
        let ids: Vec<WindowId> = mgr.durable_index().entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2]);
    }

    /// The index is a byte-comparison witness, so its ordering has to be
    /// stable — `windows` is in spawn order and `retained` in index order, and
    /// a set that merely reordered would look like a change and rewrite every
    /// frame.
    #[test]
    fn the_durable_index_is_ordered_by_id_whatever_order_windows_arrived_in() {
        let peers = test_peers();
        let pid = peers.primary_peer_id().to_string();
        let mut mgr = indexed_manager();
        mgr.adopt_index(&crate::window_index::WindowIndex {
            entries: vec![crate::window_index::WindowIndexEntry {
                id: 9,
                type_name: "Beta".into(),
                peer_id: pid.clone(),
            }],
        });
        mgr.spawn("Alpha", &pid, &peers).expect("spawn");
        let ids: Vec<WindowId> = mgr.durable_index().entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![9, 10]);
        assert_eq!(
            mgr.durable_index().to_entity().data,
            mgr.durable_index().to_entity().data,
            "the witness must be stable across calls"
        );
    }

    #[test]
    fn spawn_creates_instance() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_eq!(mgr.open_count(), 1);
        assert!(mgr.get(id).is_some());
    }

    /// **AP41's class-level enforcement point.** Every window `spawn` creates is
    /// offered the durable-hydration step, so a window type that retains a
    /// synchronous `initialize` read is repaired by overriding one trait method
    /// — not by remembering to add a call to its own factory, which is what the
    /// content-site fix originally required and what would have left the next
    /// window author to rediscover the defect.
    ///
    /// **Falsified 2026-08-31:** commenting out `view.hydrate_durable(peers)` in
    /// `spawn` reds this. That is the whole point of the test — the default
    /// implementation is a no-op, so nothing else in the suite can notice
    /// whether the call site exists.
    /// **Five outcomes, five words** — the guard against re-merging any two of
    /// them at the reporting layer.
    ///
    /// `AlreadyResolved` and `Superseded` *were* one variant, and the D13 line
    /// printed `superseded` for a healthy Direct-arm boot where the
    /// construction read simply answered — the same word it prints when the
    /// user's own navigation beat the read. An incident reading that log could
    /// not tell the normal path from the guard firing. This is AP40's rule in
    /// its cheapest enforceable form: a collapsed value is only split once, and
    /// the split has to stay split.
    ///
    /// It is a label test rather than a semantic one because the semantics are
    /// pinned where they happen — `a_shell_whose_sync_read_answered_does_no_round_trip`
    /// and `a_surface_the_sync_read_hydrated_does_no_round_trip` assert
    /// `AlreadyResolved`; `a_navigation_during_the_read_is_not_clobbered` and
    /// `state_changed_during_the_read_is_not_clobbered` assert `Superseded`.
    /// What no test covered was the two of them collapsing back into one
    /// *report*, which is where the defect actually lived.
    #[test]
    fn every_hydration_outcome_has_its_own_word() {
        let all = [
            Hydration::Adopted,
            Hydration::NonePersisted,
            Hydration::Unheard,
            Hydration::Superseded,
            Hydration::AlreadyResolved,
        ];
        let mut labels: Vec<&str> = all.iter().map(|h| h.label()).collect();
        let before = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            before,
            "two hydration outcomes report the same word, so the D13 line cannot \
             tell them apart: {labels:?}"
        );
        assert_eq!(
            before, 5,
            "a hydration outcome was added or removed without updating this test — \
             which is the test's job, since a new outcome with a duplicated label \
             would otherwise pass"
        );
        // The one fact a caller may branch on stays exactly one fact.
        assert!(Hydration::Adopted.adopted());
        for h in all.iter().filter(|h| **h != Hydration::Adopted) {
            assert!(!h.adopted(), "{h:?} must not read as the user's state reaching the screen");
        }
    }

    #[test]
    fn spawn_offers_every_window_the_durable_hydration_step() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CALLS: AtomicUsize = AtomicUsize::new(0);

        struct HydratingView {
            watch: crate::window_watch::WindowWatch,
        }
        impl WindowView for HydratingView {
            fn title(&self) -> String {
                "Hydrating".into()
            }
            fn type_name(&self) -> &'static str {
                "Hydrating"
            }
            fn peer_id(&self) -> &str {
                ""
            }
            fn watch(&self) -> &crate::window_watch::WindowWatch {
                &self.watch
            }
            fn hydrate_durable(&self, _peers: &Peers) {
                CALLS.fetch_add(1, Ordering::SeqCst);
            }
            fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}
            #[cfg(target_arch = "wasm32")]
            fn render_dom(
                &self,
                _c: &web_sys::Element,
                _s: &Peers,
                _x: &crate::dom::util::DomCtx,
            ) {
            }
        }

        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(WindowType {
            name: "Hydrating",
            description: "Test window that records its hydration",
            scope: WindowScope::System,
            create: |_id, _peer_id, _pm| {
                Box::new(HydratingView {
                    watch: crate::window_watch::WindowWatch::new(),
                })
            },
        });

        let before = CALLS.load(Ordering::SeqCst);
        mgr.spawn("Hydrating", peers.primary_peer_id(), &peers)
            .unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            before + 1,
            "spawn must offer the durable-hydration step to the window it created"
        );

        // And once per window, not once per manager: a second spawn is a second
        // surface with its own un-hydrated read to correct.
        mgr.spawn("Hydrating", peers.primary_peer_id(), &peers)
            .unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), before + 2);
    }

    /// ⭐ **The aim is offered by `spawn`, and the ORDER is the property.**
    ///
    /// A no-op default means nothing else in the suite can see whether the hook
    /// is wired, so the call site is gated here — the same reason
    /// `spawn_offers_every_window_the_durable_hydration_step` exists. The second
    /// half is the one that would be silently wrong: an aim applied *before*
    /// `hydrate_durable` is inside the witness that guard reads, so a persisted
    /// location would land after it and overwrite what the caller asked for.
    /// Recording the order of the two calls is the only way a native test can
    /// see it (the hydration round-trip itself is `cfg(wasm32)`).
    #[test]
    fn spawn_offers_the_aim_to_the_window_it_created_and_does_it_after_hydrating() {
        use std::cell::RefCell;
        thread_local! {
            static ORDER: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
        }

        struct AimableView {
            watch: crate::window_watch::WindowWatch,
            seen: Option<String>,
        }
        impl WindowView for AimableView {
            fn title(&self) -> String {
                "Aimable".into()
            }
            fn type_name(&self) -> &'static str {
                "Aimable"
            }
            fn peer_id(&self) -> &str {
                ""
            }
            fn watch(&self) -> &crate::window_watch::WindowWatch {
                &self.watch
            }
            fn hydrate_durable(&self, _peers: &Peers) {
                ORDER.with(|o| o.borrow_mut().push("hydrate"));
            }
            fn aim(&mut self, target: &crate::entity_ref::EntityRef, _peers: &Peers) -> Aim {
                ORDER.with(|o| o.borrow_mut().push("aim"));
                self.seen = Some(target.peer().to_string());
                Aim::Aimed
            }
            fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}
            #[cfg(target_arch = "wasm32")]
            fn render_dom(
                &self,
                _c: &web_sys::Element,
                _s: &Peers,
                _x: &crate::dom::util::DomCtx,
            ) {
            }
        }

        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(WindowType {
            name: "Aimable",
            description: "Test window that records being aimed",
            scope: WindowScope::System,
            create: |_id, _peer_id, _pm| {
                Box::new(AimableView {
                    watch: crate::window_watch::WindowWatch::new(),
                    seen: None,
                })
            },
        });

        let target = crate::open_target::feed("PUBLISHER");
        mgr.spawn_at("Aimable", peers.primary_peer_id(), Some(&target), &peers)
            .unwrap();
        ORDER.with(|o| {
            assert_eq!(
                o.borrow().as_slice(),
                ["hydrate", "aim"],
                "the aim must run AFTER hydrate_durable, or the persisted state \
                 the in-flight read carries lands on top of the address the \
                 caller asked for"
            );
            o.borrow_mut().clear();
        });

        // …and a spawn with no address does not aim at all. A window opened from
        // the palette has no subject, and inventing one is exactly the guess
        // (AP54) this seam exists to remove.
        mgr.spawn("Aimable", peers.primary_peer_id(), &peers).unwrap();
        ORDER.with(|o| assert_eq!(o.borrow().as_slice(), ["hydrate"]));
    }

    /// Four outcomes, four words, and the count asserted — so a fifth cannot
    /// quietly reuse one. Same shape and same reason as
    /// `every_hydration_outcome_has_its_own_word`.
    #[test]
    fn every_aim_outcome_has_its_own_word() {
        let all = [
            Aim::NotAimable,
            Aim::Aimed,
            Aim::NotMine,
            Aim::Unusable("names no site"),
        ];
        let words: std::collections::BTreeSet<&str> = all.iter().map(|a| a.label()).collect();
        assert_eq!(words.len(), all.len(), "two aim outcomes share a word: {words:?}");
        assert_eq!(all.len(), 4, "an outcome was added or removed without a word");
        // Only one of them is a hit, and it is the one that means the window is
        // showing what was asked for.
        assert_eq!(all.iter().filter(|a| a.hit()).count(), 1);
    }

    #[test]
    fn spawn_unknown_type_returns_none() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        assert!(mgr.spawn("NonExistent", peers.primary_peer_id(), &peers).is_none());
    }

    #[test]
    fn spawn_multiple_instances() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id3 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_eq!(mgr.open_count(), 3);
    }

    #[test]
    fn close_marks_not_open() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id);
        assert!(!mgr.get(id).unwrap().open);
        assert_eq!(mgr.open_count(), 0);
    }

    #[test]
    fn gc_removes_closed() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let _id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id1);
        mgr.gc_closed();
        assert_eq!(mgr.windows.len(), 1);
        assert!(mgr.get(id1).is_none());
    }

    #[test]
    fn find_open_matches_type_and_peer() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let pid = peers.primary_peer_id();
        let id = mgr.spawn("Dummy", pid, &peers).unwrap();

        // DummyView::type_name() == "Dummy", peer_id() == "" (default).
        assert_eq!(mgr.find_open("Dummy", ""), Some(id));
        assert_eq!(mgr.find_open("Other", ""), None);
        assert_eq!(mgr.find_open("Dummy", "some-other-peer"), None);

        // A closed window is not "open" for focus purposes.
        mgr.close(id);
        assert_eq!(mgr.find_open("Dummy", ""), None);
    }

    #[test]
    fn ids_are_unique_and_increasing() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn close_and_respawn_gets_new_id() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id1);
        mgr.gc_closed();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_ne!(id1, id2);
        assert!(id2 > id1);
    }

    #[test]
    fn multiple_types_registered() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(WindowType {
            name: "TypeA",
            description: "",
            scope: WindowScope::System,
            create: |_, _, _| Box::new(DummyView { name: "A".into(), watch: crate::window_watch::WindowWatch::new() }),
        });
        mgr.register_type(WindowType {
            name: "TypeB",
            description: "",
            scope: WindowScope::Peer,
            create: |_, _, _| Box::new(DummyView { name: "B".into(), watch: crate::window_watch::WindowWatch::new() }),
        });
        let a = mgr.spawn("TypeA", peers.primary_peer_id(), &peers).unwrap();
        let b = mgr.spawn("TypeB", peers.primary_peer_id(), &peers).unwrap();
        assert_eq!(mgr.get(a).unwrap().view.title(), "A");
        assert_eq!(mgr.get(b).unwrap().view.title(), "B");
        assert_eq!(mgr.open_count(), 2);
    }

    #[test]
    fn get_mut_allows_modification() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.get_mut(id).unwrap().open = false;
        assert_eq!(mgr.open_count(), 0);
    }

    #[tokio::test]
    async fn handle_action_dispatches_to_correct_window() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(
            crate::views::entity_tree::EntityTreeWindow::window_type(),
        );
        let id1 = mgr.spawn("Entity Tree", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Entity Tree", peers.primary_peer_id(), &peers).unwrap();

        // Navigate window 1 only.
        let action = Action::Navigate(id1, "docs/test".into());
        if let Some(win) = mgr.get_mut(id1) {
            win.view.handle_action(&action, &peers);
        }
        // Window state writes go through L1 dispatch; let the spawned
        // put task complete before reading the tree.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Window 1 navigated (state in tree), window 2 unchanged.
        let pid = peers.primary_peer_id();
        let e1 = peers.get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, id1));
        let e2 = peers.get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, id2));
        assert!(e1.is_some());
        assert!(e2.is_some());
        // Window 1 state should contain the navigated path.
        let data1 = crate::format::format_entity_data(&e1.unwrap().data);
        assert!(data1.contains("docs/test"), "window 1 should have navigated path");
        // Window 2 state should not contain a current_path.
        let data2 = crate::format::format_entity_data(&e2.unwrap().data);
        assert!(!data2.contains("current_path"), "window 2 should have no current_path");
    }

    /// **Window ids are reused, and window state outlives the window that
    /// wrote it.** [`WindowManager::new`] restarts `next_id` at 1 every
    /// session, and a reload is not a close — only `Action::CloseWindow`
    /// removes window state — so the entity at `workspace/windows/1/state`
    /// on this boot was written by whatever window held id 1 on the last
    /// one, of **any** type.
    ///
    /// Measured before this guard existed: the Entity Tree and the Knowledge
    /// Base both persist `expanded_paths` and mean different things by it, so
    /// decoding by field name alone made one adopt the other's. Every
    /// window-state decoder must reject a foreign `entity_type` and answer
    /// "no persisted state" instead.
    ///
    /// This is the enforcement point for the class, not for one pair: a new
    /// window type that persists a field name an existing one already uses is
    /// otherwise a silent cross-type read waiting to happen.
    ///
    /// Every row carries its own **control** — the same bytes under the row's
    /// own type must still decode — so a payload that decodes to nothing
    /// cannot make this pass vacuously (AP39).
    #[test]
    fn no_window_state_decoder_adopts_another_window_types_entity() {
        use entity_entity::Entity;

        // (label, entity written by that window type, "did a decode adopt it?")
        type Probe = Box<dyn Fn(&Entity) -> bool>;
        let rows: Vec<(&str, Entity, Probe)> = vec![
            (
                crate::views::shell::model::STATE_TYPE,
                {
                    let mut s = crate::views::shell::model::ShellState::initial("PEER");
                    s.wd = "/PEER/docs/".into();
                    s.to_entity()
                },
                // The shell's "no persisted state" answer is `initial("")`,
                // not `Default` — compare against that, not a literal.
                Box::new(|e| {
                    crate::views::shell::model::ShellState::from_entity(e).wd
                        != crate::views::shell::model::ShellState::initial("").wd
                }),
            ),
            (
                crate::views::entity_tree::model::STATE_TYPE,
                crate::views::entity_tree::model::EntityTreeState {
                    current_path: Some("/PEER/a/b".into()),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::entity_tree::model::EntityTreeState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::knowledge_base::model::STATE_TYPE,
                crate::views::knowledge_base::model::KnowledgeBaseState {
                    expanded_paths: vec!["guides".into()],
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::knowledge_base::model::KnowledgeBaseState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::query_console::model::STATE_TYPE,
                crate::views::query_console::model::QueryState {
                    path_prefix: "/PEER/x".into(),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::query_console::model::QueryState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::execute_console::model::STATE_TYPE,
                crate::views::execute_console::model::ExecuteState {
                    resource: "/PEER/x".into(),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::execute_console::model::ExecuteState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::peer_connections::model::STATE_TYPE,
                crate::views::peer_connections::model::PeerConnectionsState {
                    address: "ws://elsewhere:9999".into(),
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::peer_connections::model::PeerConnectionsState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::chain_trace::model::STATE_TYPE,
                crate::views::chain_trace::model::ChainTraceState {
                    chain_id: "chain-1".into(),
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::chain_trace::model::ChainTraceState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::theme_editor::model::STATE_TYPE,
                crate::views::theme_editor::model::ThemeEditorState {
                    editing: "midnight".into(),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::theme_editor::model::ThemeEditorState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::content_site::model::STATE_TYPE,
                crate::views::content_site::model::ContentSiteState {
                    site_id: "blog".into(),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::content_site::model::ContentSiteState::from_entity(e)
                        != Default::default()
                }),
            ),
            (
                crate::views::games::APP_VIEW_TYPE,
                crate::views::games::AppViewState {
                    selected: "games/chess".into(),
                    ..Default::default()
                }
                .to_entity(),
                Box::new(|e| {
                    crate::views::games::AppViewState::from_entity(e) != Default::default()
                }),
            ),
            // Programs shares AppViewState's SHAPE with the Apps window above
            // and must not share its SLOT: same fields, different vocabulary
            // for `selected`, both at `window_state_path` under a reused id.
            (
                crate::views::programs::PROGRAMS_VIEW_TYPE,
                crate::views::games::AppViewState {
                    selected: "life".into(),
                    ..Default::default()
                }
                .to_entity_as(crate::views::programs::PROGRAMS_VIEW_TYPE),
                Box::new(|e| {
                    crate::views::games::AppViewState::from_entity_as(
                        e,
                        crate::views::programs::PROGRAMS_VIEW_TYPE,
                    ) != Default::default()
                }),
            ),
        ];

        // The roster is the denominator: every window type that persists state
        // at `window_state_path` needs a row here, and the list was derived by
        // grep once already and came up three short (`content_site`, `games`,
        // `programs`). If you add a window with persisted state, add its row.
        assert_eq!(
            rows.len(),
            11,
            "window-state decoder census changed — re-run \
             `grep -rln window_state_path src/views/` and add the missing rows"
        );

        // **Distinct types, asserted first.** The cross-check below skips
        // `writer == reader`, so two window types that SHARE a state type are
        // invisible to it — and that is not hypothetical: Programs reused the
        // Apps window's `app/state/games_view` at its own reused-id path, and
        // the first version of this test passed with them merged. A guard
        // cannot separate two readers that agree on their type; only distinct
        // types can.
        let mut seen: Vec<&str> = Vec::new();
        for (label, _, _) in &rows {
            assert!(
                !seen.contains(label),
                "two window types persist state as `{label}` at \
                 `window_state_path`, whose id is reused across a reload — the \
                 type guard cannot tell them apart. Give one its own type"
            );
            seen.push(label);
        }

        for (label, own, probe) in &rows {
            assert_eq!(
                own.entity_type, *label,
                "{label}: to_entity must stamp the declared STATE_TYPE"
            );
            assert!(
                probe(own),
                "{label}: control failed — its OWN populated state does not \
                 decode, so the cross-type assertions below prove nothing"
            );
        }

        // The adversarial case, deliberately stronger than a realistic one:
        // give each reader ITS OWN payload — every field it looks for, all
        // decodable — stamped with another window type. Only the type field
        // can refuse it, so all 56 pairs falsify the guard rather than just
        // the `expanded_paths` pair that occurs naturally today.
        for (reader, own, probe) in &rows {
            for (writer, _, _) in &rows {
                if writer == reader {
                    continue;
                }
                let foreign = Entity::new(*writer, own.data.clone()).unwrap();
                assert!(
                    !probe(&foreign),
                    "{reader} adopted state stamped {writer} — window ids are \
                     reused across a reload, so this is reachable by opening \
                     {reader} first in the next session"
                );
            }
        }
    }
}
