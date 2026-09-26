fn main() {
    println!("cargo:rerun-if-changed=packaging/volna.rc");
    println!("cargo:rerun-if-changed=assets/app-icon/volna.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // GPUI loads the executable's icon resource with numeric ID 1.
        // Fail if the Windows resource compiler is missing, rather than
        // silently shipping an executable with the default Windows icon.
        embed_resource::compile_for("packaging/volna.rc", ["volna"], embed_resource::NONE)
            .manifest_required()
            .expect("compile the Volna application icon");
    }
}
