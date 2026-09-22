//! Percent-encoding — **one expression of the mechanism, two policies at the
//! call sites** (RFC 3986 §2.1).
//!
//! This exists because the second caller was about to be written. The first is
//! [`crate::session_config`]'s query reader, whose policy on a malformed escape
//! is *"hand the resolver the original bytes"* — deliberate, documented there,
//! and **fail-closed for something a person typed into a URL bar**. The second
//! is [`crate::entity_ref`]'s reference parser, whose policy is the opposite:
//! `APP-CONVENTION-REFERENCE` **`REF-R20`** requires an unparseable reference to
//! be *refused* rather than tolerantly re-anchored, because *"a tolerant
//! re-anchoring scan produces a well-formed wrong location and cannot report
//! that it did."*
//!
//! **Those are two policies over one mechanism, and the split is where C15's
//! rule puts it:** the decoder reports, the caller decides. A second private
//! `fn percent_decode` in the reference module would have been the fourth
//! expression of a cache rule wearing a different hat.
//!
//! ## The encoder's set is `REF-R14`'s set
//!
//! [`encode_component`] escapes everything outside RFC 3986 §2.3's *unreserved*
//! set (`ALPHA / DIGIT / "-" / "." / "_" / "~"`). That is deliberately wider
//! than "the characters that would break parsing": `REF-R14` says **reserved**
//! characters within a path segment MUST be percent-encoded, and encoding a
//! superset of what is strictly necessary is what makes re-serialization
//! *canonical* — which is the only way `REF-R8`'s byte-identical round trip can
//! hold between two implementations that disagree about what "necessary" means.
//!
//! **Uppercase hex digits**, per RFC 3986 §2.1 and §6.2.2.1. A decoder accepts
//! either case; an encoder that emitted lowercase could not round-trip a string
//! another implementation emitted.

use std::borrow::Cow;

/// Why a percent-decode refused.
///
/// Kept as a typed reason rather than an `Option` because the two arms route
/// differently: a malformed escape is a defect in the string, and a non-UTF-8
/// result is a well-formed escape sequence naming bytes that are not text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PercentError {
    /// A `%` not followed by two hex digits.
    MalformedEscape,
    /// The decoded bytes are not valid UTF-8.
    NotUtf8,
}

impl std::fmt::Display for PercentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PercentError::MalformedEscape => write!(f, "malformed percent-escape"),
            PercentError::NotUtf8 => write!(f, "percent-escape decoded to non-UTF-8 bytes"),
        }
    }
}

/// Decode `%XX` escapes, **refusing** a malformed one.
///
/// Borrows when there is nothing to decode, which is the common case (a Base58
/// peer id needs no escaping).
pub fn decode(s: &str) -> Result<Cow<'_, str>, PercentError> {
    if !s.contains('%') {
        return Ok(Cow::Borrowed(s));
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() {
                return Err(PercentError::MalformedEscape);
            }
            match (
                (b[i + 1] as char).to_digit(16),
                (b[i + 2] as char).to_digit(16),
            ) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                _ => return Err(PercentError::MalformedEscape),
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out)
        .map(Cow::Owned)
        .map_err(|_| PercentError::NotUtf8)
}

/// Decode `%XX` escapes, **yielding the original on a malformed one**.
///
/// This is [`crate::session_config`]'s policy and the reason it is spelled out
/// rather than inlined: that reader feeds a fail-closed resolver, and handing
/// it undecodable bytes is a better outcome than a panic on a URL somebody
/// typed. **Do not reach for this in a parser that owes a refusal** — see the
/// module doc.
pub fn decode_lenient(s: &str) -> Cow<'_, str> {
    match decode(s) {
        Ok(decoded) => decoded,
        Err(_) => Cow::Borrowed(s),
    }
}

/// Whether a byte may appear literally — RFC 3986 §2.3's *unreserved* set.
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// Percent-encode everything outside the unreserved set, uppercase hex.
///
/// Borrows when nothing needs encoding. **A `/` is encoded** — this escapes one
/// *component* (a single path segment, a query value, one anchor field name),
/// so a delimiter appearing inside a component is data, not structure. The
/// caller joins the encoded components with the delimiter.
pub fn encode_component(s: &str) -> Cow<'_, str> {
    if s.bytes().all(is_unreserved) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_borrows_when_there_is_nothing_to_do() {
        assert!(matches!(decode("plain-text").unwrap(), Cow::Borrowed(_)));
    }

    #[test]
    fn decode_handles_ordinary_escapes() {
        assert_eq!(decode("a%20b").unwrap(), "a b");
        assert_eq!(decode("%2F").unwrap(), "/");
        // Case-insensitive on parse, per RFC 3986 §6.2.2.1.
        assert_eq!(decode("%2f").unwrap(), "/");
    }

    /// The two policies, side by side — this is the whole reason the module
    /// exists, so it is asserted rather than described.
    #[test]
    fn a_malformed_escape_is_refused_strictly_and_tolerated_leniently() {
        for bad in ["100%", "%zz", "%a"] {
            assert_eq!(
                decode(bad),
                Err(PercentError::MalformedEscape),
                "strict decode must refuse {bad:?}"
            );
            assert_eq!(decode_lenient(bad), bad, "lenient decode must yield {bad:?}");
        }
    }

    #[test]
    fn a_plus_is_not_a_space() {
        // Form encoding is a different specification. RFC 3986 has no `+` rule.
        assert_eq!(decode("a+b").unwrap(), "a+b");
        assert_eq!(decode_lenient("a+b"), "a+b");
    }

    #[test]
    fn non_utf8_escapes_are_their_own_refusal() {
        assert_eq!(decode("%FF%FE"), Err(PercentError::NotUtf8));
    }

    #[test]
    fn encode_escapes_everything_outside_the_unreserved_set() {
        assert_eq!(encode_component("plain-text_1.0~x"), "plain-text_1.0~x");
        assert_eq!(encode_component("a b"), "a%20b");
        assert_eq!(encode_component("a/b"), "a%2Fb");
        assert_eq!(encode_component("a#b"), "a%23b");
        assert_eq!(encode_component("a?b=c&d"), "a%3Fb%3Dc%26d");
    }

    /// **Uppercase**, because a lowercase emitter could not reproduce another
    /// implementation's bytes and `REF-R8` is a cross-implementation claim.
    #[test]
    fn encoded_hex_digits_are_uppercase() {
        assert_eq!(encode_component("/"), "%2F");
        assert!(!encode_component("~ /").contains("%2f"));
    }

    #[test]
    fn encode_then_decode_round_trips() {
        for s in ["a b", "a/b", "", "ünïcøde", "100%", "%zz", "a?b=c#d"] {
            let encoded = encode_component(s);
            assert_eq!(decode(&encoded).unwrap(), s, "round trip failed for {s:?}");
        }
    }
}
