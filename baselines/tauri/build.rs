fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "kiri_marker",
                "kiri_echo",
                "kiri_ipc_bench_done",
            ])),
    )
    .expect("failed to run tauri-build");
}
