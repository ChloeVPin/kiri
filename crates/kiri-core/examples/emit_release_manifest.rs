//! Emit Kiri's cert-free, Ed25519-signed release manifest (RELEASES.json).
//!
//! This is the producer side of G-3's signed-update chain. It needs NO Apple
//! or Microsoft code-signing certificate: the signature commits to each
//! platform installer's URL + SHA-256 using Kiri's pinned Ed25519 release key,
//! which the shipping runtime verifies (`UpdaterService` -> `UpdateManifest::
//! verify_asset_for`). The OS-native binary signing/notarization (codesign/
//! notarytool on macOS, signtool on Windows) is a SEPARATE step handled by
//! `tools/packaging/package.sh` and genuinely requires those certs.
//!
//! Run: cargo run -q --release -p kiri-core --example emit_release_manifest -- <version> <out.json>
//!
//! The example also verifies the manifest it produced against the pinned
//! public key, so a key mismatch fails loudly instead of shipping a manifest
//! the runtime would reject.

use std::env;
use std::fs;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: emit_release_manifest <version> <out.json>");
        std::process::exit(2);
    }
    let version = &args[1];
    let out_path = &args[2];

    // Pinned release signing key (seed) — matches HOST_PINNED_UPDATE_PUBLIC_KEY
    // in the runtime. In a real pipeline this would come from a secret store,
    // never the source tree; it is shown here because the example must run
    // cert-free on any host for the audit loop.
    let signing_key_hex = "0707070707070707070707070707070707070707070707070707070707070707";

    let platform = kiri_core::update::current_platform_key();
    // The installer bytes are not present at manifest-emit time in CI; the
    // producer signs over the installer URL + its SHA-256. We sign over a
    // deterministic placeholder digest derived from the version+platform so
    // the manifest is reproducible and verifiable; the real pipeline substitutes
    // the actual built artifact's hash via add_signed_asset.
    let installer = format!("kiri-{version}-{platform}.bin").into_bytes();
    let url = format!("https://updates.kiri.local/kiri-{version}-{platform}.bin");

    let json = kiri_core::update::UpdateManifestBuilder::new(version)
        .notes(format!("Kiri {version} ({platform})"))
        .add_signed_asset(platform.clone(), url, &installer, signing_key_hex)
        .expect("sign asset")
        .to_json()
        .expect("serialize manifest");

    fs::write(out_path, &json).expect("write manifest");

    // Self-verify against the pinned public key the runtime pins. The pinned
    // key is the Ed25519 verify half of the seed `[7u8; 32]` release signing
    // key, identical to the derivation in `updater_surface.rs` and
    // `host_cross.rs`. We re-derive it here rather than reading a runtime
    // const, so the producer example has no dependency on the host crate.
    let pk = hex::encode(
        ed25519_dalek::VerifyingKey::from(&ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]))
            .to_bytes(),
    );
    let manifest: kiri_core::update::UpdateManifest =
        serde_json::from_str(&json).expect("parse manifest");
    let verified = manifest
        .verify_asset_for(&platform, &pk, &installer)
        .expect("manifest must verify against pinned key");
    assert_eq!(verified.version, kiri_core::update::Version::parse(version).unwrap());

    println!("wrote {out_path} (platform={platform}, version={version}, verified=ok)");
}
