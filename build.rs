fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Recompile when localization files change, so the `i18n!` macro
    // (which embeds `locales/*.yml` at compile time) picks up edits.
    println!("cargo:rerun-if-changed=locales");

    // Generate the gRPC client for the user-service microservice from the
    // vendored proto contract (the `user-service-proto` git submodule).
    // Requires `protoc` to be available at build time.
    let proto = "user-service-proto/service.proto";
    if std::path::Path::new(proto).exists() {
        println!("cargo:rerun-if-changed=user-service-proto");
        tonic_prost_build::configure()
            .build_server(false)
            .compile_protos(&[proto], &["user-service-proto"])?;
    } else {
        // The submodule isn't checked out (e.g. cargo-chef's dependency-only "cook"
        // stage, which only reconstructs manifests). Skip codegen here; the real
        // build has the submodule and runs it. Remember to `git submodule update --init`.
        println!("cargo:warning=user-service-proto/service.proto not found; skipping gRPC codegen (run `git submodule update --init`)");
    }

    Ok(())
}
