fn main() {
    // Recompile when localization files change, so the `i18n!` macro
    // (which embeds `locales/*.yml` at compile time) picks up edits.
    println!("cargo:rerun-if-changed=locales");
}
