fn main() {
    println!("cargo:rerun-if-changed=src/trace/printf.c");
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() != Ok("wasm") {
        cc::Build::new()
            .file("src/trace/printf.c")
            .compile("vtr_printf");
    }
}
