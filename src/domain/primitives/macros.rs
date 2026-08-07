/// Builds a domain value out of a literal, checked while the code is compiled.
///
/// `literal!(Counter = 0)` reads as the constant declaration it expands into, and that is the whole
/// point: a `const` block forces the type's validator to run at build time, so a value the type
/// would refuse never reaches a running bot. Written as a plain `Counter::literal(0)` the same
/// validator only runs when the line is reached — a `const fn` is evaluated early solely where the
/// language requires it, and an ordinary call requires nothing.
///
/// Beware of where the failure shows up: an inline `const` block is evaluated during codegen, which
/// `cargo check` skips. `cargo build` and `cargo test` report it; the IDE stays quiet.
///
/// The `#[allow]` is what lets `clippy.toml` forbid the bare constructor everywhere else: the lint
/// doesn't skip macro expansions, it reports them here, so silencing it once here covers every use
/// of the macro and nothing besides.
#[macro_export]
macro_rules! literal {
    ($type:ty = $value:expr) => {
        const {
            #[allow(clippy::disallowed_methods)]
            <$type>::literal($value)
        }
    };
}

#[macro_export]
macro_rules! error {
    ($name:ident) => {
        #[derive(Debug, derive_more::Error, derive_more::Display)]
        pub struct $name(#[error(not(source))] String);
        
        impl $name {
            pub fn message(msg: impl ToString) -> Self {
                Self(msg.to_string())
            }
        }
    };
}

/// A domain number: a wrapper over an integer, with the arithmetic that goes with it.
///
/// A quantity that can never be negative takes an unsigned inner type, which says so in the type
/// system rather than in a validator — there is then no invalid value to refuse, so the constructor
/// and the arithmetic stay infallible. Postgres has no unsigned column, so the macro stores such a
/// type in the signed integer of the same width.
#[macro_export]
macro_rules! number {
    ($name:ident, $inner_type:ty) => {
        #[domain_type(number)]
        struct $name($inner_type);
    };
}
