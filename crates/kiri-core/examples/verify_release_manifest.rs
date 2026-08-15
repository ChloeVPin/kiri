//! Verify every real artifact referenced by a merged Kiri RELEASES.json.
//!
//! Usage:
//! verify_release_manifest <manifest.json> <platform=artifact>...

use std::collections::BTreeSet;
use std::env;
use std::fs;

const HOST_PINNED_UPDATE_PUBLIC_KEY: &str =
    "333d58ae1e42ba2025b035666528d36430e0c14e13f3d5006c7f0fe22a9d3af6";

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: verify_release_manifest <manifest.json> <platform=artifact>...");
        std::process::exit(2);
    }

    let manifest_path = &args[1];
    let manifest_json = fs::read_to_string(manifest_path).expect("read manifest");
    let manifest =
        kiri_core::update::UpdateManifest::parse_json(&manifest_json).expect("parse manifest");
    let mut verified_platforms = BTreeSet::new();

    let mut verified = 0usize;
    for pair in &args[2..] {
        let (platform, artifact_path) = pair.split_once('=').expect("platform=artifact pair");
        let bytes = fs::read(artifact_path).expect("read artifact");
        manifest
            .verify_asset_for(platform, HOST_PINNED_UPDATE_PUBLIC_KEY, &bytes)
            .expect("artifact must match its signed manifest entry");
        assert!(
            verified_platforms.insert(platform.to_string()),
            "platform supplied more than once"
        );
        verified += 1;
        println!("verified {platform}: {artifact_path} ({} bytes)", bytes.len());
    }

    assert_eq!(verified, manifest.platforms.len(), "every manifest platform must be verified");
    assert_eq!(
        verified_platforms,
        manifest.platforms.keys().cloned().collect(),
        "verification pairs must cover exactly the manifest platforms"
    );
    println!("verified {verified} platform assets");
}
