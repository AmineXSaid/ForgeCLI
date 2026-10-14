//! On Windows, the Forge cube becomes forge.exe's icon (`assets/forge.rc`).
//! A build without a resource compiler fails here rather than ship no icon.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/forge.rc");
        println!("cargo:rerun-if-changed=assets/forge.ico");
        embed_resource::compile("assets/forge.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the forge icon (assets/forge.rc) could not be compiled");
    }
}
