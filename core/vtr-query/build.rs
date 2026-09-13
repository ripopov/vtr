fn main() {
    #[cfg(feature = "wire")]
    {
        println!("cargo:rerun-if-changed=schema/query.proto");
        prost_build::Config::new()
            .compile_protos(&["schema/query.proto"], &["schema"])
            .expect("compile query protocol (requires protoc)");
    }
}
