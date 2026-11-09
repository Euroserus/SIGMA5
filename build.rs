fn main() {
    // Only the release script builds a plain version (SIGMA5_RELEASE=1).
    // Everything else is a "-dev" build, which counts as older than the
    // release of the same number, so a test build can never block an update.
    println!("cargo:rerun-if-env-changed=SIGMA5_RELEASE");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let release = std::env::var("SIGMA5_RELEASE").is_ok_and(|v| v == "1");
    let label = if release { version } else { format!("{version}-dev") };
    println!("cargo:rustc-env=SIGMA5_VERSION={label}");

    println!("cargo:rerun-if-changed=assets/sigma5.rc");
    println!("cargo:rerun-if-changed=assets/sigma5.ico");
    println!("cargo:rerun-if-changed=assets/sigma5.manifest");
    embed_resource::compile("assets/sigma5.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
