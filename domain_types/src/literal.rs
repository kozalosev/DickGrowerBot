/// Builds a value of a **validated** domain type out of a literal, checked while the code is
/// compiled. A type that validates nothing has no `check_literal`; its `new` is `const` and
/// infallible, which is all a constant needs.
///
/// `literal!(Ratio = 0.5)` reads as the constant declaration it expands into, and that is the whole
/// point: a `const` block forces the validator to run at build time, so a value the type would
/// refuse never reaches a running program. Written as a plain `Ratio::from_literal(0.5)` nothing is
/// checked at all — a `const fn` is evaluated early solely where the language requires it, and an
/// ordinary call requires nothing.
///
/// Beware of where the failure shows up: an inline `const` block is evaluated during codegen, which
/// `cargo check` skips. `cargo build` and `cargo test` report it; the IDE stays quiet.
///
/// The `#[allow]` travels with the expansion, so a project that forbids the bare constructor needs
/// no exception for the one call that is allowed to make it.
#[macro_export]
macro_rules! literal {
    ($type:ty = $value:expr) => {{
        #[allow(clippy::disallowed_methods)]
        let checked = <$type>::from_literal(const { <$type>::check_literal($value) });
        checked
    }};
}
