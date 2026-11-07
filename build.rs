fn main() {
    println!("cargo:rerun-if-changed=assets/sigma5.rc");
    println!("cargo:rerun-if-changed=assets/sigma5.ico");
    println!("cargo:rerun-if-changed=assets/sigma5.manifest");
    embed_resource::compile("assets/sigma5.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
