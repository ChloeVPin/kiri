//! Emit Kiri's Ed25519-signed release manifest for one real artifact.
//!
//! The signature is an application-level integrity check and does not require
//! Apple Developer or Microsoft code-signing certificates. The producer reads
//! the bytes of the artifact that will be published, signs its URL and SHA-256,
//! writes `RELEASES.json`, and verifies the result before returning success.
//!
//! Run:
//! cargo run -q --release -p kiri-core --example emit_release_manifest -- \
//!   <version> <platform> <url> <artifact> <out.json>

use std::env;
use std::fs;

use ed25519_dalek::SigningKey;

const HOST_PINNED_UPDATE_PUBLIC_KEY: &str =
    "333d58ae1e42ba2025b035666528d36430e0c14e13f3d5006c7f0fe22a9d3af6";

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 6 {
        eprintln!("usage: emit_release_manifest <version> <platform> <url> <artifact> <out.json>");
        std::process::exit(2);
    }
    let version = &args[1];
    let platform = &args[2];
    let url = &args[3];
    let artifact_path = &args[4];
    let out_path = &args[5];

    kiri_core::update::Version::parse(version).expect("version must be valid semver");
    if !url.starts_with("https://") {
        eprintln!("release asset URL must use https://");
        std::process::exit(2);
    }
    let artifact = fs::read(artifact_path).expect("read release artifact");

    let signing_key_hex = env::var("KIRI_UPDATE_SIGNING_KEY_HEX").unwrap_or_else(|_| {
        eprintln!("KIRI_UPDATE_SIGNING_KEY_HEX is required and must stay outside source control");
        std::process::exit(2);
    });

    let json = kiri_core::update::UpdateManifestBuilder::new(version)
        .notes(format!("Kiri {version} ({platform})"))
        .add_signed_asset(platform.clone(), url.clone(), &artifact, &signing_key_hex)
        .expect("sign asset")
        .to_json()
        .expect("serialize manifest");

    fs::write(out_path, &json).expect("write manifest");

    let manifest = kiri_core::update::UpdateManifest::parse_json(&json).expect("parse manifest");
    let verify_key = if env::var("KIRI_ALLOW_TEST_UPDATE_KEY").as_deref() == Ok("1") {
        let raw = hex::decode(&signing_key_hex).expect("decode signing key for rehearsal");
        if raw.len() != 32 {
            eprintln!("KIRI_UPDATE_SIGNING_KEY_HEX must be 32 bytes hex");
            std::process::exit(2);
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&raw);
        let sk = SigningKey::from_bytes(&seed);
        hex::encode(sk.verifying_key().to_bytes())
    } else {
        HOST_PINNED_UPDATE_PUBLIC_KEY.to_string()
    };
    let verified = manifest
        .verify_asset_for(platform, &verify_key, &artifact)
        .expect("manifest must verify against pinned key");
    assert_eq!(verified.version, kiri_core::update::Version::parse(version).unwrap());

    println!(
        "wrote {out_path} (platform={platform}, version={version}, bytes={}, verified=ok)",
        artifact.len()
    );
}
