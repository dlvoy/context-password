fn main() {
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.manifest");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("resources/app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed resources/app.rc");
    }
}
