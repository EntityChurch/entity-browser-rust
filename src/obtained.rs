//! **An entity you did not author travels as bytes; a struct is only a view of
//! it.** [`Obtained<T>`] carries both, so a decode can never silently become a
//! rewrite.
//!
//! ## The rule
//!
//! *If someone else gives you an entity, that is what you store.* Not your
//! re-encoding of the fields you happened to model — **their bytes**. Rewriting
//! is a thing you may decide to do, deliberately, at a site that says so; it is
//! never something a `to_entity()` call does on your behalf because the decoded
//! struct was the only thing still in scope.
//!
//! ## Why this is a type and not a convention
//!
//! It was a convention, and it decayed exactly the way AP44 says a convention
//! decays. [`crate::content_site::foreign_cache::ensure_current`] — written for
//! D24, which is *about* holding somebody else's bytes — gets it right by
//! carrying the [`Entity`] itself. [`crate::feed_mirror::plan_mirror`] gets it
//! right, and says so in a comment (*"the obtained entity, cloned. Never
//! `row.entry.to_entity()`"*). The content-site resolver's write-through, which
//! is older than both, got it wrong at three call sites in one function — while
//! its own doc comment claimed the writes were *"byte-faithful"*.
//!
//! Nobody was careless. The decoded struct is what a renderer needs, so it is
//! what a resolver returns, and by the time anything wants to *store* the value
//! the bytes are three stack frames gone. **The fix is to make the bytes
//! impossible to lose, not to remember them.** A `.to_entity()` on an
//! `Obtained<T>` does not compile; [`Obtained::entity`] is the only way out, and
//! it answers with the author's bytes whenever there are any.
//!
//! ## What was measured, 2026-09-16
//!
//! Three arms, through the real `SiteManifest` codec:
//!
//! | the publisher emits | we store | |
//! |---|---|---|
//! | a field we do not model (`theme_hint`) | 52 B → **30 B**, field gone | ✗ |
//! | exactly the fields we model | identical | ✓ |
//! | **no `nav`** — an ordinary one-page site | 25 B → **30 B**, `nav: []` added | ✗ |
//!
//! ⭐ **The third arm is the one that decides this is a defect rather than a
//! forward-compatibility nicety: it contains no unknown field.** A single-page
//! site is conformant — `APP-CONVENTION-SEMANTIC-CONTENT-SITE` §4's CDDL makes
//! `nav` optional and the convention says *title-only is conformant* — and we
//! rewrite it anyway, because our encoder emits a key their encoder omitted.
//! Same shape as the `gpin4-joint` finding one convention over (*a markdown page
//! with no frontmatter stores `title: ""`; the `.html` path removes it — same
//! absence, two encodings*), arriving where it costs something.
//!
//! ## What it cost, and it is not the fidelity
//!
//! The stored copy lands at the **publisher's own path** in our tree, which is
//! also [`ForeignArtifact::Manifest::store_path`](crate::content_site::foreign_cache::ForeignArtifact::store_path)
//! — so D24's currency check reads it back and compares its `content_hash`
//! against the origin's hop-1 pointer. A rewritten copy's hash **can never
//! match the pointer it came from**, so `ensure_current` answers `Fetched` on
//! every sweep, forever, for every publisher whose encoding differs from ours by
//! one optional key. The 58-byte-pointer mechanism D24 exists to buy is defeated
//! — not by a missing currency trigger, which is the failure D24 was written
//! about, but by **a sibling writer at the same path putting bytes there that
//! the trigger cannot recognise.**
//!
//! ⇒ **When two code paths write the same tree key, they owe the same bytes.**
//! Availability never suffered and nothing rendered wrong, which is the whole
//! reason it survived: the symptom is a fetch that did not need to happen and a
//! ledger row that reads `Fetched` when nothing changed.
//!
//! ## `None` is a real answer, not a missing one
//!
//! [`Obtained::authored`] means *we made this value* — a synthesized section
//! index, a demo site, a page we are about to publish — and for those our
//! encoding **is** the canonical one, so `entity()` encoding from the struct is
//! correct rather than a fallback. The two cases must stay apart (AP40): *these
//! are their bytes* and *these are ours* decide different things, and a single
//! `Option<Entity>` field that readers branch on by hand would let a third
//! meaning (*we lost them*) sneak in between.

use entity_entity::Entity;
use std::ops::Deref;

/// A type that knows its own canonical entity encoding.
///
/// Implemented for the value types we both decode and author, so
/// [`Obtained::entity`] can fall back to re-encoding for the values that are
/// genuinely ours. Deliberately narrow: this is not a serialization framework,
/// it is the set of things [`Obtained`] can hold.
pub trait Encodes {
    /// This value's canonical entity, encoded from its fields.
    fn to_entity(&self) -> Entity;
}

/// A decoded value **and the bytes it was decoded from**, when those bytes came
/// from somebody else.
///
/// Derefs to the decoded value, so every read site is unchanged; the only way to
/// get an [`Entity`] back out is [`entity`](Self::entity), which prefers the
/// author's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obtained<T> {
    decoded: T,
    /// The entity as its author published it. `None` means **we** authored this
    /// value, so there is nothing to preserve and our encoding is the canonical
    /// one — never *"we had bytes and dropped them"*.
    source: Option<Entity>,
}

impl<T> Obtained<T> {
    /// **We** authored this value. Our encoding is canonical for it.
    pub fn authored(decoded: T) -> Self {
        Self { decoded, source: None }
    }

    /// Somebody else authored this entity; `decode` produces our view of it and
    /// the entity is kept so a write-back is byte-faithful.
    ///
    /// Takes the entity by value because keeping it is the point — a caller who
    /// wants to drop it has to say so by calling [`authored`](Self::authored)
    /// instead, which reads as the claim it is.
    pub fn decoded_from(source: Entity, decode: impl FnOnce(&Entity) -> T) -> Self {
        let decoded = decode(&source);
        Self { decoded, source: Some(source) }
    }

    /// The bytes as their author published them, if this value came from
    /// somebody else.
    pub fn source(&self) -> Option<&Entity> {
        self.source.as_ref()
    }

    /// Whether these are somebody else's bytes. Useful to a caller that must
    /// report which it is; **not** something a write path should branch on —
    /// use [`entity`](Self::entity), which already answers correctly for both.
    pub fn is_foreign(&self) -> bool {
        self.source.is_some()
    }

    /// Replace the decoded view, keeping the source bytes.
    ///
    /// For the one shape that legitimately needs it: a resolver that decodes an
    /// entity and then *adjusts* the view for rendering (resolving a root slug,
    /// rewriting a relative link) without claiming the publisher wrote the
    /// adjusted version.
    pub fn map_view(self, f: impl FnOnce(T) -> T) -> Self {
        Self { decoded: f(self.decoded), source: self.source }
    }

    /// The decoded view, dropping the bytes. Named to be greppable: a call site
    /// is asserting it does not need fidelity.
    pub fn into_view(self) -> T {
        self.decoded
    }
}

impl<T: Encodes> Obtained<T> {
    /// **The entity to store.** The author's own bytes when we have them, our
    /// canonical encoding when the value is ours.
    ///
    /// This is the only way to get an `Entity` out, and it is why the type
    /// exists: a write path cannot reach `to_entity()` on the decoded value
    /// without first saying [`into_view`](Self::into_view).
    pub fn entity(&self) -> Entity {
        match &self.source {
            Some(e) => e.clone(),
            None => self.decoded.to_entity(),
        }
    }
}

impl<T> Deref for Obtained<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.decoded
    }
}

impl<T: Default> Default for Obtained<T> {
    /// A default value is one **we** produced, so it has no source.
    fn default() -> Self {
        Self::authored(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in with a deliberately **lossy** decoder — it models one key and
    /// drops the rest — because that is the whole subject. A round trip through
    /// a faithful codec proves nothing here (the `feed_mirror` control-arm
    /// lesson: a fixture your own encoder produced cannot falsify a claim about
    /// somebody else's encoder).
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct Modelled {
        known: String,
    }

    impl Encodes for Modelled {
        fn to_entity(&self) -> Entity {
            let pairs =
                vec![(entity_ecf::Value::Text("known".into()), entity_ecf::text(&self.known))];
            Entity::new("test/modelled", entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs)))
                .expect("canonical")
        }
    }

    fn decode(e: &Entity) -> Modelled {
        let v: entity_ecf::Value =
            ciborium::from_reader(e.data.as_slice()).unwrap_or(entity_ecf::Value::Null);
        let mut out = Modelled::default();
        if let Some(map) = v.as_map() {
            for (k, val) in map {
                if k.as_text() == Some("known") {
                    out.known = val.as_text().unwrap_or_default().to_string();
                }
            }
        }
        out
    }

    /// A foreign entity carrying a field this build does not model. ECF orders
    /// map keys by (length, lexical) and the encoder gets no say, so the pairs
    /// are built in that order here rather than relying on insertion order.
    fn foreign_with_extra() -> Entity {
        let pairs = vec![
            (entity_ecf::Value::Text("known".into()), entity_ecf::text("value")),
            (entity_ecf::Value::Text("unmodelled".into()), entity_ecf::text("kept")),
        ];
        Entity::new("test/modelled", entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs)))
            .expect("canonical")
    }

    /// The property the type exists for: what goes back out is what came in,
    /// **including the part we could not read**.
    #[test]
    fn an_entity_we_did_not_author_is_stored_exactly_as_it_arrived() {
        let origin = foreign_with_extra();
        let held = Obtained::decoded_from(origin.clone(), decode);

        assert_eq!(held.known, "value", "the view decodes the part we model");
        assert_eq!(
            held.entity().data,
            origin.data,
            "the stored bytes are the author's bytes, verbatim"
        );
        assert_eq!(
            held.entity().content_hash,
            origin.content_hash,
            "so the address is the author's address — which is what a currency \
             check compares against"
        );
        assert!(
            String::from_utf8_lossy(&held.entity().data).contains("unmodelled"),
            "a field we do not model survives being held"
        );
    }

    /// The falsifier for the above, spelled as its own test so the two are
    /// measured apart: re-encoding from the view is what the defect did, and it
    /// really does move the address. Without this, the test above passes for a
    /// codec that happens to be lossless.
    #[test]
    fn re_encoding_from_the_view_is_what_moves_the_address() {
        let origin = foreign_with_extra();
        let view = decode(&origin);
        let rewritten = view.to_entity();

        assert_ne!(
            rewritten.content_hash, origin.content_hash,
            "re-encoding a decoded view produces a DIFFERENT entity — this is \
             the defect, pinned so the test above cannot go vacuous"
        );
        assert!(
            !String::from_utf8_lossy(&rewritten.data).contains("unmodelled"),
            "and the unmodelled field is what it lost"
        );
    }

    /// `None` is a real answer. A value we made has no bytes to preserve and our
    /// encoding is the canonical one, so `entity()` encodes rather than failing
    /// or returning an empty entity.
    #[test]
    fn a_value_we_authored_encodes_from_its_fields() {
        let ours = Obtained::authored(Modelled { known: "mine".into() });

        assert!(!ours.is_foreign(), "we made it");
        assert_eq!(ours.source(), None, "there are no author bytes to keep");
        assert_eq!(
            ours.entity().content_hash,
            Modelled { known: "mine".into() }.to_entity().content_hash,
            "our encoding IS the canonical one for a value we authored"
        );
    }

    /// Adjusting the view for rendering must not be able to claim the publisher
    /// authored the adjustment.
    #[test]
    fn adjusting_the_view_does_not_touch_the_bytes() {
        let origin = foreign_with_extra();
        let adjusted = Obtained::decoded_from(origin.clone(), decode)
            .map_view(|m| Modelled { known: format!("{}-resolved", m.known) });

        assert_eq!(adjusted.known, "value-resolved", "the view moved");
        assert_eq!(
            adjusted.entity().data,
            origin.data,
            "and the bytes did not — a render-time adjustment is not a republish"
        );
    }
}
