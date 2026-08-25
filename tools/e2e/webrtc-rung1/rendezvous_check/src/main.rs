use entity_signaling::key::pair_key;
use entity_signaling::webrtc::{glare_role, GlareRole};

fn show(a: &str, b: &str, ta: &str, tb: &str, label: &str) {
    // A dials target ta; B dials target tb. self ids are a, b.
    let ra = glare_role(a, ta).unwrap();
    let rb = glare_role(b, tb).unwrap();
    let ka = pair_key(a, ta);
    let kb = pair_key(b, tb);
    let off = |r: &GlareRole| matches!(r, GlareRole::Impolite);
    println!("--- {label} ---");
    println!("  A self={:.10} target={:.14} -> {}", a, ta, if off(&ra){"OFFERS"}else{"answers"});
    println!("  B self={:.10} target={:.14} -> {}", b, tb, if off(&rb){"OFFERS"}else{"answers"});
    println!("  BOTH offer? {}   keys match? {}",
             off(&ra)&&off(&rb), ka.as_bytes()==kb.as_bytes());
}

fn main() {
    let a58 = "2K9rhC9b3MyEsmhZkkjSAKHjQFrFdEAEREM4Ywt7fSfVuk";
    let b58 = "2KEa3DxCZvjB9gRttqfzRaPMnhzNbKYgGg38Si681ttYpj";
    let a_auth = "ecfv1-sha256:457ebb49c3118508bbd07e0cb4297ed01ba4ab3d3df5fd72913b3f3447d92aa7";
    let b_auth = "ecfv1-sha256:69d2aace7163d86f97386f2e1bb665573abd82ffff260214a6a12d6fd1eb9cbb";

    // observed at runtime: BOTH offer, keys differ (included_count=0)
    show(a58, b58, b58, a58, "self=base58, target=base58  (what the code path implies)");
    show(a58, b58, b_auth, a_auth, "self=base58, target=author-hash");
    show(a_auth, b_auth, b58, a58, "self=author-hash, target=base58");
    show(a_auth, b_auth, b_auth, a_auth, "self=author-hash, target=author-hash");
}
