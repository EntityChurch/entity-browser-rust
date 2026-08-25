//! Programs window — the generic compute-program host surface.
//!
//! Lists the embedded program fixtures (Life / Snake / Asteroids —
//! authored once by workbench-go, imported hash-verified), and per
//! program: Install (materialize + seed), Start / Stop / Restart, the
//! host status (D13: every state has a surface — materializing progress,
//! tick count, fault reason), and the bound shape driver's display.
//!
//! The clock is self-scheduling through the render loop: a tick's
//! completion (or a pending-interval sleep) marks the window dirty, the
//! rebuild gets `&Peers`, and schedules the next pipeline — the
//! "subscription-driven, don't poll" discipline applied to a clock, with
//! the eval futures created synchronously from `&Peers` per the
//! `Sdk::execute` contract (`program_host::host`).

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::program_host::bundle::{Bundle, EMBEDDED_PROGRAMS};
use crate::program_host::descriptor::{ProgramDescriptor, SHAPE_TEXT};
use crate::window_watch::WindowWatch;

/// Install (materialize + seed) a program. Value = program key.
pub const INSTALL_EVENT: &str = "program_install";
/// Start / stop / restart the host clock. Value = program key.
pub const START_EVENT: &str = "program_start";
pub const STOP_EVENT: &str = "program_stop";
pub const RESTART_EVENT: &str = "program_restart";

/// Shapes this window can drive today. Phase 1 adds `direction` +
/// `key-set`, phase 2 adds `display-list` — admission refuses the rest
/// with the shape named (the capability-set gate, workbench's pattern).
pub const SUPPORTED_SHAPES: &[&str] = &[SHAPE_TEXT];

/// Host run-state per program (D13: every state renders).
#[derive(Debug, Clone)]
pub enum MountStatus {
    /// Parsed, admitted, not yet materialized.
    Absent,
    /// Admission refused — reason shown, actions disabled.
    Refused(String),
    Materializing { done: usize, total: usize },
    Stopped,
    Running,
    Faulted(String),
}

/// One program's mount state. Bundle + descriptor are parsed at window
/// creation (decode errors surface as `Refused`); everything mutable
/// lives behind the window's shared `Rc` so spawned tick tasks can land
/// results.
pub struct Mount {
    pub bundle: Rc<Bundle>,
    pub descriptor: Option<Rc<ProgramDescriptor>>,
    pub status: MountStatus,
    pub ticks: u64,
    /// Monotonic guard: bumped on Stop/Restart so an in-flight tick's
    /// landing is dropped instead of resurrecting a cancelled clock.
    pub generation: u64,
    pub in_flight: bool,
    /// A wake-up sleep for the next tick interval is already scheduled.
    pub sleep_scheduled: bool,
    /// `performance.now()` timestamp the next tick is due.
    pub next_due_ms: f64,
}

pub type Mounts = Rc<RefCell<BTreeMap<&'static str, Mount>>>;

pub struct ProgramsWindow {
    pub window_id: WindowId,
    pub peer_id: String,
    watch: WindowWatch,
    pub mounts: Mounts,
}

impl ProgramsWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let mut mounts = BTreeMap::new();
        for p in EMBEDDED_PROGRAMS {
            let (bundle, descriptor, status) = match Bundle::parse(p.json) {
                Ok(b) => {
                    let desc_status = b
                        .entities
                        .iter()
                        .find(|e| e.path == b.descriptor_path)
                        .ok_or_else(|| "bundle has no descriptor entity".to_string()) // i18n-ignore — build-defect diagnostic, not UI prose
                        .and_then(|be| {
                            let ent = entity_entity::Entity::new(&be.entity_type, be.data.clone())
                                .map_err(|e| e.to_string())?;
                            ProgramDescriptor::decode(&ent)
                        })
                        .and_then(|d| {
                            d.admit(SUPPORTED_SHAPES)?;
                            Ok(d)
                        });
                    match desc_status {
                        Ok(d) => (Rc::new(b), Some(Rc::new(d)), MountStatus::Absent),
                        Err(reason) => (Rc::new(b), None, MountStatus::Refused(reason)),
                    }
                }
                Err(e) => {
                    // A broken embedded fixture is a build defect; surface
                    // it in place rather than hiding the row.
                    let empty = Bundle {
                        program: p.key.to_string(),
                        root: String::new(),
                        origin_peer: String::new(),
                        descriptor_path: String::new(),
                        entities: Vec::new(),
                        inputs: Vec::new(),
                        oracle: Vec::new(),
                    };
                    (Rc::new(empty), None, MountStatus::Refused(e))
                }
            };
            mounts.insert(
                p.key,
                Mount {
                    bundle,
                    descriptor,
                    status,
                    ticks: 0,
                    generation: 0,
                    in_flight: false,
                    sleep_scheduled: false,
                    next_due_ms: 0.0,
                },
            );
        }
        Self {
            window_id,
            peer_id,
            watch: WindowWatch::new(),
            mounts: Rc::new(RefCell::new(mounts)),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Programs", // i18n-ignore — identity key; display via window.programs
            description: "Run transferable compute programs (EXTENSION-COMPUTE) from their descriptors", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ProgramsWindow::new(id, peer_id.to_string());
                // Subscribe each program's ORIGIN-namespace root: ticks
                // land there, and on the Worker arm this is what primes
                // the read mirror the display driver reads from.
                let prefixes: Vec<String> = window
                    .mounts
                    .borrow()
                    .values()
                    .filter(|m| !m.bundle.origin_peer.is_empty())
                    .map(|m| format!("/{}/{}/", m.bundle.origin_peer, m.bundle.root))
                    .collect();
                for prefix in prefixes {
                    pm.watch_prefix(&mut window.watch, peer_id, prefix);
                }
                Box::new(window)
            },
        }
    }

    /// The tick interval in ms from the descriptor's `rate_hint`.
    pub fn interval_ms(desc: &ProgramDescriptor) -> f64 {
        1000.0 / (desc.tick.rate_hint.max(1) as f64)
    }
}

impl WindowView for ProgramsWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Programs") // i18n-ignore — lookup key, resolves via catalog
    }

    fn type_name(&self) -> &'static str {
        "Programs" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { event, value, .. } = action else {
            return;
        };
        let Some(key) = EMBEDDED_PROGRAMS
            .iter()
            .map(|p| p.key)
            .find(|k| k == value)
        else {
            return;
        };
        match event.as_str() {
            INSTALL_EVENT => self.install(key, peers),
            START_EVENT => self.start(key),
            STOP_EVENT => self.stop(key),
            RESTART_EVENT => self.restart(key, peers),
            _ => {}
        }
        self.watch.mark_dirty();
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        // Schedule due ticks / wake-ups before painting — the render
        // loop is the only place with `&Peers` every rebuild.
        self.pump(peers);
        crate::dom::programs::render(container, self, peers, ctx);
    }
}

impl ProgramsWindow {
    /// Install: verify entity hashes, materialize at origin-namespace
    /// paths, then seed. Status transitions are the D13 surface;
    /// verification failure or a mid-flight error lands as `Faulted`.
    fn install(&mut self, key: &'static str, peers: &Peers) {
        let mounts = self.mounts.clone();
        let flag = self.watch.flag();
        let (bundle, descriptor) = {
            let m = mounts.borrow();
            let mount = &m[key];
            if mount.descriptor.is_none()
                || matches!(
                    mount.status,
                    MountStatus::Materializing { .. } | MountStatus::Running
                )
            {
                return;
            }
            (mount.bundle.clone(), mount.descriptor.clone().unwrap())
        };

        let entities = match bundle.verified_entities() {
            Ok(e) => e,
            Err(reason) => {
                mounts.borrow_mut().get_mut(key).unwrap().status = MountStatus::Faulted(reason);
                flag.mark();
                return;
            }
        };
        let total = entities.len();
        {
            let mut m = mounts.borrow_mut();
            let mount = m.get_mut(key).unwrap();
            mount.status = MountStatus::Materializing { done: 0, total };
            mount.ticks = 0;
            mount.generation += 1;
            mount.in_flight = false;
        }

        let ns = bundle.origin_peer.clone();
        let progress_mounts = mounts.clone();
        let progress_flag = flag.clone();
        let mat = crate::program_host::host::materialize_future(
            peers,
            &self.peer_id,
            &ns,
            entities,
            move |done, total| {
                if let Some(mount) = progress_mounts.borrow_mut().get_mut(key) {
                    mount.status = MountStatus::Materializing { done, total };
                }
                // Coalesced by the frame loop; fine at fixture sizes.
                progress_flag.mark();
            },
        );
        let seed = crate::program_host::host::seed_future(peers, &self.peer_id, &ns, &descriptor);
        spawn(async move {
            let outcome = async {
                mat.await?;
                seed.await
            }
            .await;
            let mut m = mounts.borrow_mut();
            let Some(mount) = m.get_mut(key) else { return };
            mount.status = match outcome {
                Ok(()) => MountStatus::Stopped,
                Err(e) => MountStatus::Faulted(format!("install: {e}")),
            };
            flag.mark();
        });
    }

    fn start(&mut self, key: &'static str) {
        let mut m = self.mounts.borrow_mut();
        let Some(mount) = m.get_mut(key) else { return };
        if matches!(mount.status, MountStatus::Stopped) {
            mount.status = MountStatus::Running;
            mount.next_due_ms = 0.0; // due immediately; pump schedules
        }
    }

    fn stop(&mut self, key: &'static str) {
        let mut m = self.mounts.borrow_mut();
        let Some(mount) = m.get_mut(key) else { return };
        if matches!(mount.status, MountStatus::Running) {
            mount.status = MountStatus::Stopped;
            mount.generation += 1;
            mount.in_flight = false;
        }
    }

    /// Restart = stop + reseed (workbench semantics: back to state₀,
    /// stays stopped).
    fn restart(&mut self, key: &'static str, peers: &Peers) {
        let mounts = self.mounts.clone();
        let flag = self.watch.flag();
        let (ns, descriptor) = {
            let mut m = mounts.borrow_mut();
            let Some(mount) = m.get_mut(key) else { return };
            let Some(desc) = mount.descriptor.clone() else { return };
            if matches!(
                mount.status,
                MountStatus::Absent | MountStatus::Refused(_) | MountStatus::Materializing { .. }
            ) {
                return;
            }
            mount.status = MountStatus::Stopped;
            mount.generation += 1;
            mount.in_flight = false;
            mount.ticks = 0;
            (mount.bundle.origin_peer.clone(), desc)
        };
        let seed = crate::program_host::host::seed_future(peers, &self.peer_id, &ns, &descriptor);
        spawn(async move {
            let outcome = seed.await;
            let mut m = mounts.borrow_mut();
            let Some(mount) = m.get_mut(key) else { return };
            if let Err(e) = outcome {
                mount.status = MountStatus::Faulted(format!("reseed: {e}"));
            }
            flag.mark();
        });
    }

    /// Schedule due ticks and interval wake-ups. Called from render (the
    /// per-rebuild `&Peers` window). One pipeline in flight per program.
    #[cfg(target_arch = "wasm32")]
    fn pump(&self, peers: &Peers) {
        let now = crate::dom::programs::now_ms();
        let keys: Vec<&'static str> = self.mounts.borrow().keys().copied().collect();
        for key in keys {
            let (due_in, descriptor, ns, generation) = {
                let mut m = self.mounts.borrow_mut();
                let Some(mount) = m.get_mut(key) else { continue };
                if !matches!(mount.status, MountStatus::Running) || mount.in_flight {
                    continue;
                }
                let Some(desc) = mount.descriptor.clone() else { continue };
                let due_in = mount.next_due_ms - now;
                if due_in > 1.0 {
                    if mount.sleep_scheduled {
                        continue;
                    }
                    mount.sleep_scheduled = true;
                } else {
                    mount.in_flight = true;
                }
                (due_in, desc, mount.bundle.origin_peer.clone(), mount.generation)
            };

            let mounts = self.mounts.clone();
            let flag = self.watch.flag();
            if due_in > 1.0 {
                // Interval wake-up: mark dirty when the tick is due.
                tracing::debug!(program = key, due_in, "program tick sleep");
                spawn(async move {
                    crate::dom::programs::sleep_ms(due_in as i32).await;
                    if let Some(mount) = mounts.borrow_mut().get_mut(key) {
                        mount.sleep_scheduled = false;
                    }
                    flag.mark();
                });
                continue;
            }

            let interval = Self::interval_ms(&descriptor);
            let tick = crate::program_host::host::tick_future(peers, &self.peer_id, &ns, &descriptor);
            tracing::debug!(program = key, gen = generation, "program tick spawn");
            spawn(async move {
                // A tick that never lands would freeze the counter with a
                // green status — time it out into a loud Faulted instead
                // (D13: no silent stall).
                #[cfg(target_arch = "wasm32")]
                let outcome = crate::dom::programs::with_timeout(tick, 5000).await;
                #[cfg(not(target_arch = "wasm32"))]
                let outcome = tick.await;
                tracing::debug!(program = key, ok = outcome.is_ok(), "program tick landed");
                let done = crate::dom::programs::now_ms();
                let mut m = mounts.borrow_mut();
                let Some(mount) = m.get_mut(key) else { return };
                if mount.generation != generation {
                    return; // stopped/restarted mid-tick — drop the landing
                }
                mount.in_flight = false;
                match outcome {
                    Ok(()) => {
                        mount.ticks += 1;
                        // Schedule from completion when overrunning
                        // (degrade, don't pile up).
                        let next = mount.next_due_ms + interval;
                        mount.next_due_ms = if next < done { done } else { next };
                        if mount.next_due_ms == done {
                            tracing::debug!(program = key, "program tick overran its interval");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(program = key, error = %e, "program tick faulted — clock stopped");
                        mount.status = MountStatus::Faulted(e);
                    }
                }
                flag.mark();
            });
        }
    }
}

/// `spawn_local` on wasm; on native the window never runs a clock (the
/// oracle test drives `program_host` directly), so this is a no-op that
/// keeps the non-render methods compiling for the registry tests.
fn spawn<F: std::future::Future<Output = ()> + 'static>(fut: F) {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(fut);
    #[cfg(not(target_arch = "wasm32"))]
    drop(fut);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_peer_scoped() {
        let t = ProgramsWindow::window_type();
        assert_eq!(t.name, "Programs");
        assert!(matches!(t.scope, crate::window::WindowScope::Peer));
    }

    /// Phase-0 admission: Life admits on the text-only driver set;
    /// Snake/Asteroids are refused with their missing shape named (the
    /// honest-refusal gate — they unlock in phases 1/2).
    #[test]
    fn phase0_admission_partition() {
        let w = ProgramsWindow::new(1, "peer".into());
        let m = w.mounts.borrow();
        assert!(matches!(m["life"].status, MountStatus::Absent));
        match &m["snake"].status {
            MountStatus::Refused(r) => assert!(r.contains("direction"), "{r}"),
            other => panic!("snake should be refused on phase 0, got {other:?}"),
        }
        match &m["asteroids"].status {
            MountStatus::Refused(r) => {
                assert!(r.contains("key-set") || r.contains("display-list"), "{r}")
            }
            other => panic!("asteroids should be refused on phase 0, got {other:?}"),
        }
    }
}
