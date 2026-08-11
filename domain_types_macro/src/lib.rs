extern crate proc_macro;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Fields, Ident, ItemStruct, PathSegment, Type};
use syn::parse::{Parse, ParseStream};

mod kw {
    syn::custom_keyword!(number);
    syn::custom_keyword!(validated);
    syn::custom_keyword!(error_message);
    syn::custom_keyword!(features);
    syn::custom_keyword!(no_auto_display);
    syn::custom_keyword!(division_result);
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum IntegerSignedness {
    Signed,
    Unsigned
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum PrimitiveKind {
    Integer(IntegerSignedness),
    Float,
}

/// The three axes that used to be flattened into `NumberKind`'s 7 variants, kept orthogonal:
/// which primitive it wraps, whether it has arithmetic operators (a "number") as opposed to
/// just being an id-like value, and whether it's range-validated.
#[derive(PartialEq, Eq)]
struct NumberKind {
    primitive: PrimitiveKind,
    is_number: bool,
    validated: bool,
}

#[derive(PartialEq, Eq)]
enum DomainTypeKind {
    Number(NumberKind),
    String { validated: bool },
}

enum InnerTypeKind {
    Unsupported,
    Integer(IntegerSignedness),
    Float,
    String,
}

/// How a domain type maps onto `sqlx`. Postgres has no unsigned wire type, so an unsigned inner
/// type can't `#[sqlx(transparent)]`-derive directly; it goes through a signed type instead (see
/// [`signed_counterpart`]), which the generated `Encode`/`Decode` convert to and from.
enum SqlxMode {
    Transparent,
    Signed(Box<Type>),
}

struct TypeInfo<'a> {
    name: &'a Ident,
    inner_type: Type,
    variant: DomainTypeKind,
    args: DomainTypeAttr,
    sqlx_mode: SqlxMode,
}

struct DomainTypeAttr {
    number: bool,
    no_auto_display: bool,
    validator: Option<syn::Expr>,
    error_msg: Option<syn::LitStr>,
    division_result: Option<syn::Path>,
}

impl Parse for DomainTypeAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut number = false;
        let mut validator = None;
        let mut error_msg = None;
        let mut no_auto_display = false;
        let mut division_result = None;

        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(kw::number) {
                input.parse::<kw::number>()?;
                number = true;
            }
            else if lookahead.peek(kw::validated) {
                input.parse::<kw::validated>()?;
                let (v, msg) = parse_validated(input)?;
                validator = Some(v);
                error_msg = Some(msg);
            }
            else if lookahead.peek(kw::division_result) {
                input.parse::<kw::division_result>()?;
                let content;
                syn::parenthesized!(content in input);
                division_result = Some(content.parse()?);
            }
            else if lookahead.peek(kw::features) {
                input.parse::<kw::features>()?;
                no_auto_display = parse_features(input)?;
            }
            else {
                return Err(lookahead.error());
            }

            // Parse optional comma
            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(Self {
            number,
            no_auto_display,
            validator,
            error_msg,
            division_result,
        })
    }
}

/// Parses the `(validator_fn, error_message("..."))` payload of `validated(...)`.
fn parse_validated(input: ParseStream) -> syn::Result<(syn::Expr, syn::LitStr)> {
    let content;
    syn::parenthesized!(content in input);

    let validator = content.parse()?;
    content.parse::<syn::Token![,]>()?;

    content.parse::<kw::error_message>()?;
    let msg_content;
    syn::parenthesized!(msg_content in content);
    let error_msg = msg_content.parse()?;
    content.parse::<syn::Token![,]>().ok();

    Ok((validator, error_msg))
}

/// Parses the comma-separated flag list inside `features(...)`.
fn parse_features(input: ParseStream) -> syn::Result<bool> {
    let content;
    syn::parenthesized!(content in input);

    let mut no_auto_display = false;
    while !content.is_empty() {
        let feature_lookahead = content.lookahead1();
        if feature_lookahead.peek(kw::no_auto_display) {
            content.parse::<kw::no_auto_display>()?;
            no_auto_display = true;
        } else {
            return Err(feature_lookahead.error());
        }
        if !content.is_empty() {
            content.parse::<syn::Token![,]>()?;
        }
    }
    Ok(no_auto_display)
}

/// The signed integer type an unsigned one is stored as, for [`SqlxMode::Signed`].
///
/// The width is kept, so a domain type's column stays the column it already is: `u16` goes into an
/// `int2`, `u32` into an `int4`, `u64` into an `int8`. Only `u8` widens, because Postgres has no
/// one-byte integer. Half of the unsigned range has no signed counterpart, which is why encoding
/// converts rather than casts — but a value in that half would not have fit the column either.
fn signed_counterpart(ty: &Type) -> Option<Type> {
    if let Type::Group(group) = ty {
        return signed_counterpart(&group.elem);
    }
    let Type::Path(type_path) = ty else { return None };
    let ident = &type_path.path.segments.last()?.ident;
    let signed = match ident.to_string().as_str() {
        "u8" | "u16" => "i16",
        "u32" => "i32",
        "u64" => "i64",
        _ => return None,
    };
    syn::parse_str(signed).ok()
}

/// The name of the primitive, for the two decisions below that depend on which one it is.
fn primitive_name(ty: &Type) -> Option<String> {
    if let Type::Group(group) = ty {
        return primitive_name(&group.elem);
    }
    let Type::Path(type_path) = ty else { return None };
    Some(type_path.path.segments.last()?.ident.to_string())
}

/// The types every value of this one converts into without losing anything — std's `From` impls,
/// which is also what decides the other half: a pair absent here is one `SaturatingInto` keeps,
/// because it is a pair that can lose something.
///
/// Only the widths this workspace uses are listed. A target std has no `From` for would fail to
/// compile in [`generate_exact_widening_impls`], which is what keeps this honest.
fn exact_widenings(ty: &Type) -> Vec<Type> {
    let targets: &[&str] = match primitive_name(ty).as_deref() {
        Some("u8") => &["u16", "u32", "u64", "usize", "i16", "i32", "i64", "isize", "f32", "f64"],
        Some("u16") => &["u32", "u64", "usize", "i32", "i64", "f32", "f64"],
        Some("u32") => &["u64", "i64", "f64"],
        Some("i8") => &["i16", "i32", "i64", "isize", "f32", "f64"],
        Some("i16") => &["i32", "i64", "isize", "f32", "f64"],
        Some("i32") => &["i64", "f64"],
        Some("f32") => &["f64"],
        _ => &[],
    };
    targets.iter().filter_map(|name| syn::parse_str(name).ok()).collect()
}

/// `From<#name> for T` for each of them, so that a conversion which loses nothing says so.
///
/// The orphan rule is satisfied by the local type sitting in the trait's parameter, which is why
/// the target may be a foreign primitive.
fn generate_exact_widening_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    let impls = exact_widenings(inner_type).into_iter().map(|target| quote! {
        #[automatically_derived]
        impl ::std::convert::From<#name> for #target {
            fn from(value: #name) -> Self {
                <#target as ::std::convert::From<#inner_type>>::from(value.0)
            }
        }
    });
    quote! { #(#impls)* }
}

/// Whether every value of this type is exactly representable in an `f64`. An `f64` holds every
/// 32-bit integer and every `f32`; it does not hold every `i64` or `u64`, whose values run past its
/// 53 bits of mantissa. The exact ones widen with `From`, the rest have to name what they lose.
fn exact_in_f64(ty: &Type) -> bool {
    primitive_name(ty).is_some_and(|name|
        matches!(name.as_str(), "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "f32"))
}

#[proc_macro_attribute]
pub fn domain_type(args: proc_macro::TokenStream, input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let args = parse_macro_input!(args as DomainTypeAttr);
    let input = parse_macro_input!(input as ItemStruct);
    let name = &input.ident;

    // Extract the inner type from the tuple struct
    let inner_type = match &input.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            fields.unnamed.first().unwrap().ty.clone()
        }
        _ => panic!("domain_type can only be used for tuple structs with exactly one field")
    };

    let variant = match determine_inner_type_kind(&inner_type) {
        InnerTypeKind::Integer(signedness) => DomainTypeKind::Number(NumberKind {
            primitive: PrimitiveKind::Integer(signedness),
            is_number: args.number,
            validated: args.validator.is_some(),
        }),
        InnerTypeKind::Float => DomainTypeKind::Number(NumberKind {
            primitive: PrimitiveKind::Float,
            is_number: args.number,
            validated: args.validator.is_some(),
        }),
        InnerTypeKind::String => DomainTypeKind::String { validated: args.validator.is_some() },
        InnerTypeKind::Unsupported => panic!("unsupported domain type"),
    };

    let is_integer_number = matches!(&variant,
        DomainTypeKind::Number(NumberKind { primitive: PrimitiveKind::Integer(_), is_number: true, .. })
    );
    if args.division_result.is_some() && !is_integer_number {
        panic!("division_result is only applicable to integer domain numbers")
    }

    let sqlx_mode = match &variant {
        DomainTypeKind::Number(NumberKind { primitive: PrimitiveKind::Integer(IntegerSignedness::Unsigned), .. }) => {
            let signed = signed_counterpart(&inner_type)
                .unwrap_or_else(|| panic!("no signed type to store {} in", quote!(#inner_type)));
            SqlxMode::Signed(Box::new(signed))
        }
        _ => SqlxMode::Transparent,
    };

    let info = TypeInfo {
        name, inner_type, args, variant, sqlx_mode,
    };
    let derives = generate_derives(&info);
    let impls = generate_impls(&info);

    let (sqlx_transparent, signed_sqlx_impls) = match &info.sqlx_mode {
        SqlxMode::Transparent => (quote! { #[sqlx(transparent)] }, TokenStream::new()),
        SqlxMode::Signed(signed) => (quote! {}, generate_signed_sqlx_impls(&info, signed)),
    };

    let TypeInfo { name, inner_type, .. } = info;
    // Everything written above the struct is carried over: its documentation, and any attribute
    // the derives below read — `#[display(...)]` above all, which `derive_more::Display` takes.
    let attrs = &input.attrs;
    // Generate the final struct with a conditional 'sqlx' attribute
    let output = quote! {
        #(#attrs)*
        #[derive(#(#derives),*)]
        #sqlx_transparent
        pub struct #name(#inner_type);

        #impls
        #signed_sqlx_impls
    };

    proc_macro::TokenStream::from(output)
}

/// Hand-written `Type`/`Encode`/`Decode` for an unsigned inner type, going through `signed` (see
/// [`signed_counterpart`]). `#[sqlx(transparent)]` can't be used here since it requires the wrapped
/// field's own type to already implement these traits, with no substitution; unsigned integers never
/// do, as Postgres has no unsigned wire type.
///
/// Both directions convert instead of casting, and both can refuse: a value above the signed half
/// of the range going out, a negative one coming back. Neither is reachable through a column that
/// holds what this type writes.
fn generate_signed_sqlx_impls(info: &TypeInfo, signed: &Type) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        #[automatically_derived]
        impl<DB: ::sqlx::Database> ::sqlx::Type<DB> for #name
        where #signed: ::sqlx::Type<DB>
        {
            fn type_info() -> DB::TypeInfo {
                <#signed as ::sqlx::Type<DB>>::type_info()
            }

            fn compatible(ty: &DB::TypeInfo) -> bool {
                <#signed as ::sqlx::Type<DB>>::compatible(ty)
            }
        }

        #[automatically_derived]
        impl<'q, DB: ::sqlx::Database> ::sqlx::Encode<'q, DB> for #name
        where #signed: ::sqlx::Encode<'q, DB>
        {
            fn encode_by_ref(
                &self,
                buf: &mut <DB as ::sqlx::Database>::ArgumentBuffer,
            ) -> ::std::result::Result<::sqlx::encode::IsNull, ::sqlx::error::BoxDynError> {
                let value = <#signed as ::std::convert::TryFrom<#inner_type>>::try_from(self.0)?;
                <#signed as ::sqlx::Encode<DB>>::encode_by_ref(&value, buf)
            }
        }

        #[automatically_derived]
        impl<'r, DB: ::sqlx::Database> ::sqlx::Decode<'r, DB> for #name
        where #signed: ::sqlx::Decode<'r, DB>
        {
            fn decode(
                value: <DB as ::sqlx::Database>::ValueRef<'r>,
            ) -> ::std::result::Result<Self, ::sqlx::error::BoxDynError> {
                let raw = <#signed as ::sqlx::Decode<DB>>::decode(value)?;
                Ok(Self(<#inner_type as ::std::convert::TryFrom<#signed>>::try_from(raw)?))
            }
        }

        // What `#[sqlx(transparent)]` would have derived. Without it the type can be bound on its
        // own but not as an array, which is how a batch insert passes its rows.
        #[automatically_derived]
        impl ::sqlx::postgres::PgHasArrayType for #name {
            fn array_type_info() -> ::sqlx::postgres::PgTypeInfo {
                <#signed as ::sqlx::postgres::PgHasArrayType>::array_type_info()
            }
        }
    }
}

/// The two conversions that lose something, forwarded to the inner type.
///
/// Both are bounded by what the inner type itself serves, so each impl covers exactly the targets
/// that make sense for this wrapper and no branching is needed here: a float-backed type ends up
/// with `ApproxInto<f32>`, an integer-backed one with `SaturatingInto<i64>`. Which end a value is
/// clamped to, and how a float is rounded, is decided once in `domain_types::traits`.
///
/// The lossless direction is `From<#name> for #inner_type`, generated with the other basics.
fn generate_lossy_conversion_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        #[automatically_derived]
        impl<T> ::domain_types::traits::SaturatingInto<T> for #name
        where #inner_type: ::domain_types::traits::SaturatingInto<T>
        {
            fn saturating_into(self) -> T {
                ::domain_types::traits::SaturatingInto::saturating_into(self.0)
            }
        }

        #[automatically_derived]
        impl<T> ::domain_types::traits::ApproxInto<T> for #name
        where #inner_type: ::domain_types::traits::ApproxInto<T>
        {
            fn approx_into(self) -> T {
                ::domain_types::traits::ApproxInto::approx_into(self.0)
            }
        }
    }
}

fn generate_derives(info: &TypeInfo) -> Vec<TokenStream> {
    let mut derives = vec![
        quote! { Clone },
        quote! { Debug },
        quote! { ::serde::Serialize },
        quote! { Default },
        quote! { PartialEq },
        quote! { PartialOrd },
    ];

    // Validated types can't derive `Deserialize`: the transparent newtype derive would
    // construct `Self(value)` directly, bypassing the range validator (the same hazard as
    // `Neg` and the plain `FromStr`). They get a hand-written impl routing through `Self::new`
    // in `generate_validated_domain_number_impls` instead.
    let is_validated = matches!(&info.variant,
        DomainTypeKind::Number(NumberKind { validated: true, .. }) | DomainTypeKind::String { validated: true });
    if !is_validated {
        derives.push(quote! { ::serde::Deserialize });
    }

    match &info.variant {
        DomainTypeKind::String { .. } => {
            derives.push(quote! { Eq });
            derives.push(quote! { Ord });
            derives.push(quote! { Hash });
        }
        DomainTypeKind::Number(kind) => {
            derives.push(quote! { Copy });
            if matches!(kind.primitive, PrimitiveKind::Integer(_)) {
                derives.push(quote! { Eq });
                derives.push(quote! { Ord });
                derives.push(quote! { Hash });
                // Validated types never derive Neg: it would construct the negated value
                // directly, bypassing the validator (e.g. -Page(1) would produce an invalid
                // Page(-1)). Unsigned integers can't derive Neg either (no `-` on the inner type).
                if !kind.validated && kind.primitive == PrimitiveKind::Integer(IntegerSignedness::Signed) {
                    derives.push(quote! { ::derive_more::Neg });
                }
            }
            // Arithmetic operators for float numbers are generated as explicit impls
            // (see generate_domain_float_number_impls), not derived: derive_more's op derives
            // don't produce the `Op<T>` / `Op<Self>` combination the DomainNumber trait requires.
        }
    }

    if matches!(info.sqlx_mode, SqlxMode::Transparent) {
        derives.push(quote! { ::sqlx::Type })
    }
    if !info.args.no_auto_display {
        derives.push(quote! { ::derive_more::Display })
    }

    derives
}

fn generate_domain_value_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, variant, .. } = info;
    // Validated kinds must route through the fallible constructor instead of `Self(value)`
    // directly, or FromStr would silently bypass the validator.
    let is_validated = matches!(variant, DomainTypeKind::Number(NumberKind { validated: true, .. }));
    let from_str_impl = if is_validated {
        quote! {
            #[automatically_derived]
            impl ::std::str::FromStr for #name {
                type Err = ::domain_types::errors::DomainParseError;

                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    #inner_type::from_str(s)
                        .map_err(Box::new)
                        .map_err(|err| ::domain_types::errors::DomainParseError::new(s.to_owned(), stringify!(#name), err))
                        .and_then(|value| Self::new(value)
                            .map_err(Box::new)
                            .map_err(|err| ::domain_types::errors::DomainParseError::new(s.to_owned(), stringify!(#name), err)))
                }
            }
        }
    } else {
        quote! {
            #[automatically_derived]
            impl ::std::str::FromStr for #name {
                type Err = ::domain_types::errors::DomainParseError;

                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    #inner_type::from_str(s)
                        .map(Self)
                        .map_err(Box::new)
                        .map_err(|err| ::domain_types::errors::DomainParseError::new(s.to_owned(), stringify!(#name), err))
                }
            }
        }
    };
    quote! {
        impl #name {
            pub const fn value(&self) -> #inner_type {
                self.0
            }

            pub fn is_zero(&self) -> bool {
                ::num_traits::Zero::is_zero(&self.0)
            }
        }

        #[automatically_derived]
        impl std::ops::Deref for #name {
            type Target = #inner_type;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        #[automatically_derived]
        impl ::domain_types::traits::DomainValue<#inner_type> for #name {
            fn value(&self) -> #inner_type {
                self.0
            }
        }

        #from_str_impl

        #[automatically_derived]
        impl ::std::cmp::PartialEq<#inner_type> for #name {
            fn eq(&self, other: &#inner_type) -> bool {
                <Self as ::domain_types::traits::DomainValue<#inner_type>>::value(self) == *other
            }
        }

        #[automatically_derived]
        impl ::std::cmp::PartialOrd<#inner_type> for #name {
            fn partial_cmp(&self, other: &#inner_type) -> Option<::std::cmp::Ordering> {
                <Self as ::domain_types::traits::DomainValue<#inner_type>>::value(self).partial_cmp(other)
            }
        }
    }
}

fn generate_validated_domain_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, args, .. } = info;
    let validator = args.validator.as_ref()
        .expect("Validator must be provided to generate a constructor");
    let error_msg = args.error_msg.as_ref()
        .expect("Error message must be provided to generate a constructor");
    quote! {
        impl #name {
            pub fn new(value: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                if #validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(::domain_types::errors::DomainAssertionError::new(
                        value,
                        ::std::borrow::Cow::from(concat!(stringify!(#name), ' ', #error_msg))
                    ))
                }
            }

            /// Validates and hands the value straight back. `literal!` calls this inside a `const`
            /// block, which is what makes a value the type would refuse fail the build.
            pub const fn check_literal(value: #inner_type) -> #inner_type {
                assert!(#validator(&value), #error_msg);
                value
            }

            /// Wraps what [`Self::check_literal`] approved. Never call it directly — on its own it
            /// skips the check entirely, which is why `clippy.toml` forbids it.
            pub const fn from_literal(value: #inner_type) -> Self {
                Self(value)
            }
        }

        // Hand-written instead of derived so deserialization can't bypass the validator:
        // the inner value is read transparently, then routed through the fallible `Self::new`.
        #[automatically_derived]
        impl<'de> ::serde::Deserialize<'de> for #name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let value = <#inner_type as ::serde::Deserialize>::deserialize(deserializer)?;
                Self::new(value).map_err(::serde::de::Error::custom)
            }
        }

        #[automatically_derived]
        impl ::domain_types::traits::ValidatedDomainNumber<#inner_type> for #name {
            fn new(value: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                Self::new(value)
            }
        }
    }
}

/// One of the five arithmetic operators domain numbers support, driving the codegen in
/// `generate_domain_integer_number_impls`/`generate_domain_float_number_impls` so each operator
/// isn't spelled out by hand per int/float × validated/unvalidated combination.
#[derive(Clone, Copy)]
enum ArithmeticOp { Add, Sub, Mul, Div, Rem }

impl ArithmeticOp {
    fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Sub => "sub",
            Self::Mul => "mul",
            Self::Div => "div",
            Self::Rem => "rem",
        }
    }

    fn trait_ident(self) -> Ident {
        format_ident!("{}", match self {
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::Mul => "Mul",
            Self::Div => "Div",
            Self::Rem => "Rem",
        })
    }

    fn method_ident(self) -> Ident {
        format_ident!("{}", self.name())
    }

    fn assign_trait_ident(self) -> Ident {
        format_ident!("{}Assign", self.trait_ident())
    }

    fn assign_method_ident(self) -> Ident {
        format_ident!("{}_assign", self.name())
    }

    /// The literal infix operator token. Safe to splice into generated code without importing
    /// the corresponding `std::ops` trait: unlike a `.method()` call, operator syntax always
    /// resolves regardless of what's in scope at the macro's call site.
    fn token(self) -> TokenStream {
        match self {
            Self::Add => quote! { + },
            Self::Sub => quote! { - },
            Self::Mul => quote! { * },
            Self::Div => quote! { / },
            Self::Rem => quote! { % },
        }
    }

    fn assign_token(self) -> TokenStream {
        match self {
            Self::Add => quote! { += },
            Self::Sub => quote! { -= },
            Self::Mul => quote! { *= },
            Self::Div => quote! { /= },
            Self::Rem => quote! { %= },
        }
    }

    fn overflow_variant(self) -> TokenStream {
        let variant = match self {
            Self::Add => quote! { Addition },
            Self::Sub => quote! { Subtraction },
            Self::Mul => quote! { Multiplication },
            Self::Div => quote! { Division },
            Self::Rem => quote! { Remainder },
        };
        quote! { ::domain_types::errors::ArithmeticOperation::#variant }
    }
}

/// Generates `overflowing_<op>[_primitive]`, and (if `with_saturating`) `saturating_<op>[_primitive]`,
/// for one operator. Shared between the validated and unvalidated integer paths: `validated`
/// only changes the return type (`Self`/`(Self, bool)` vs `Result<Self, DomainAssertionError<T>>`)
/// and whether construction routes through the range validator.
fn generate_integer_op_methods(inner_type: &Type, validated: bool, op: ArithmeticOp, with_saturating: bool) -> TokenStream {
    let overflowing_method = format_ident!("overflowing_{}", op.name());
    let overflowing_method_primitive = format_ident!("overflowing_{}_primitive", op.name());
    let overflow_variant = op.overflow_variant();

    let overflowing_output = if validated {
        quote! { Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> }
    } else {
        quote! { (Self, bool) }
    };
    let overflowing_body = if validated {
        quote! { Self::perform_arithmetic_operation(self.0, rhs, #overflow_variant, #inner_type::#overflowing_method) }
    } else {
        quote! {
            let (new_value, is_overflow) = self.0.#overflowing_method(rhs);
            (Self(new_value), is_overflow)
        }
    };

    let overflowing_impl = quote! {
        pub fn #overflowing_method_primitive(self, rhs: #inner_type) -> #overflowing_output {
            #overflowing_body
        }

        pub fn #overflowing_method(self, rhs: Self) -> #overflowing_output {
            self.#overflowing_method_primitive(rhs.0)
        }
    };

    if !with_saturating {
        return overflowing_impl;
    }

    let saturating_method = format_ident!("saturating_{}", op.name());
    let saturating_method_primitive = format_ident!("saturating_{}_primitive", op.name());
    let output = if validated {
        quote! { Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> }
    } else {
        quote! { Self }
    };
    let saturating_body = if validated {
        quote! { <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.#saturating_method(rhs)) }
    } else {
        quote! { Self(self.0.#saturating_method(rhs)) }
    };

    quote! {
        #overflowing_impl

        pub fn #saturating_method_primitive(self, rhs: #inner_type) -> #output {
            #saturating_body
        }

        pub fn #saturating_method(self, rhs: Self) -> #output {
            self.#saturating_method_primitive(rhs.0)
        }
    }
}

fn generate_integer_operator_impl(name: &Ident, inner_type: &Type, validated: bool, op: ArithmeticOp) -> TokenStream {
    let trait_ident = op.trait_ident();
    let method = op.method_ident();
    let saturating_method_primitive = format_ident!("saturating_{}_primitive", op.name());
    let saturating_method = format_ident!("saturating_{}", op.name());
    let output = if validated {
        quote! { Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> }
    } else {
        quote! { Self }
    };

    quote! {
        #[automatically_derived]
        impl std::ops::#trait_ident<#inner_type> for #name {
            type Output = #output;

            fn #method(self, rhs: #inner_type) -> Self::Output {
                self.#saturating_method_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::#trait_ident for #name {
            type Output = #output;

            fn #method(self, rhs: Self) -> Self::Output {
                self.#saturating_method(rhs)
            }
        }
    }
}

fn generate_integer_assign_operator_impl(name: &Ident, inner_type: &Type, op: ArithmeticOp) -> TokenStream {
    let trait_ident = op.assign_trait_ident();
    let method = op.assign_method_ident();
    let saturating_method = format_ident!("saturating_{}", op.name());

    quote! {
        #[automatically_derived]
        impl std::ops::#trait_ident<#inner_type> for #name {
            fn #method(&mut self, rhs: #inner_type) {
                self.0 = self.0.#saturating_method(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::#trait_ident for #name {
            fn #method(&mut self, rhs: Self) {
                self.0 = self.0.#saturating_method(rhs.0);
            }
        }
    }
}

/// Integer arithmetic for both unvalidated (saturating, infallible) and validated (range-checked,
/// fallible) domain numbers. `validated` selects between the two; see `ArithmeticOp` for how each
/// operator's methods/operator impls are generated from one shared template.
fn generate_domain_integer_number_impls(info: &TypeInfo, validated: bool) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;

    let perform_arithmetic_operation = if validated {
        quote! {
            fn perform_arithmetic_operation(
                lhs: #inner_type, rhs: #inner_type,
                op_enum: ::domain_types::errors::ArithmeticOperation,
                op_func: fn(#inner_type, #inner_type) -> (#inner_type, bool)
            ) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let (new_value, overflow) = op_func(lhs, rhs);
                if !overflow {
                    <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(new_value)
                } else {
                    let cause = ::domain_types::errors::DomainArithmeticOverflowError::new(op_enum, lhs, rhs);
                    let cause_boxed_str = ::std::borrow::Cow::from(cause.to_string());
                    Err(::domain_types::errors::DomainAssertionError::new(new_value, cause_boxed_str))
                }
            }
        }
    } else {
        TokenStream::new()
    };

    // Integer division producing `Self`. For a division producing a float domain type,
    // annotate the type with `division_result(...)` and use the `/` operator instead.
    let op_methods: TokenStream = [ArithmeticOp::Add, ArithmeticOp::Sub, ArithmeticOp::Mul, ArithmeticOp::Div].into_iter()
        .map(|op| generate_integer_op_methods(inner_type, validated, op, true))
        .collect();
    // No `saturating_rem`: std doesn't provide one either (remainder can only overflow on
    // `MIN % -1`, which `overflowing_rem` reports explicitly).
    let rem_methods = generate_integer_op_methods(inner_type, validated, ArithmeticOp::Rem, false);

    let operators: TokenStream = [ArithmeticOp::Add, ArithmeticOp::Sub, ArithmeticOp::Mul].into_iter()
        .map(|op| generate_integer_operator_impl(name, inner_type, validated, op))
        .collect();
    // Validated arithmetic can fail (range check), so it can't implement `*Assign`, which must
    // be infallible.
    let assign_operators: TokenStream = if validated {
        TokenStream::new()
    } else {
        [ArithmeticOp::Add, ArithmeticOp::Sub, ArithmeticOp::Mul].into_iter()
            .map(|op| generate_integer_assign_operator_impl(name, inner_type, op))
            .collect()
    };

    quote! {
        impl #name {
            #perform_arithmetic_operation
            #op_methods
            #rem_methods
        }

        #operators
        #assign_operators
    }
}

fn generate_float_operator_impl(name: &Ident, inner_type: &Type, validated: bool, op: ArithmeticOp) -> TokenStream {
    let trait_ident = op.trait_ident();
    let method = op.method_ident();
    let token = op.token();
    let output = if validated {
        quote! { Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> }
    } else {
        quote! { Self }
    };
    let primitive_body = if validated {
        quote! { <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 #token rhs) }
    } else {
        quote! { Self(self.0 #token rhs) }
    };

    quote! {
        #[automatically_derived]
        impl std::ops::#trait_ident<#inner_type> for #name {
            type Output = #output;

            fn #method(self, rhs: #inner_type) -> Self::Output {
                #primitive_body
            }
        }

        #[automatically_derived]
        impl std::ops::#trait_ident for #name {
            type Output = #output;

            fn #method(self, rhs: Self) -> Self::Output {
                self #token rhs.0
            }
        }
    }
}

fn generate_float_assign_operator_impl(name: &Ident, inner_type: &Type, op: ArithmeticOp) -> TokenStream {
    let trait_ident = op.assign_trait_ident();
    let method = op.assign_method_ident();
    let assign_token = op.assign_token();

    quote! {
        #[automatically_derived]
        impl std::ops::#trait_ident<#inner_type> for #name {
            fn #method(&mut self, rhs: #inner_type) {
                self.0 #assign_token rhs;
            }
        }

        #[automatically_derived]
        impl std::ops::#trait_ident for #name {
            fn #method(&mut self, rhs: Self) {
                self.0 #assign_token rhs.0;
            }
        }
    }
}

/// Float arithmetic for both unvalidated (infallible) and validated (range-checked, fallible)
/// domain numbers. Unlike integers there's no overflow to detect, so operators go straight
/// through the primitive operation; `validated` only changes whether the result is wrapped in
/// `Self` directly or routed through the range validator.
fn generate_domain_float_number_impls(info: &TypeInfo, validated: bool) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;

    let operators: TokenStream = [ArithmeticOp::Add, ArithmeticOp::Sub, ArithmeticOp::Mul, ArithmeticOp::Div, ArithmeticOp::Rem].into_iter()
        .map(|op| generate_float_operator_impl(name, inner_type, validated, op))
        .collect();
    // Validated arithmetic can fail (range check), so it can't implement `*Assign`, which must
    // be infallible. `Rem` has no assign counterpart either way (mirrors integer arithmetic,
    // which also only assigns Add/Sub/Mul).
    let assign_operators: TokenStream = if validated {
        TokenStream::new()
    } else {
        [ArithmeticOp::Add, ArithmeticOp::Sub, ArithmeticOp::Mul, ArithmeticOp::Div].into_iter()
            .map(|op| generate_float_assign_operator_impl(name, inner_type, op))
            .collect()
    };

    quote! {
        #operators
        #assign_operators
    }
}

/// For integer domain numbers annotated with `division_result(SomeFloatDomainType)`:
/// the `/` operator performs a float division and produces the specified float domain type
/// (or a `Result` of it, if that type is validated — see the `DivisionResult` trait).
// TODO: an f64 holds integers exactly only up to 2^53, so a 64-bit domain number divides at
//       reduced precision; consider rejecting `division_result` on i64/u64 at expansion time.
fn generate_division_operator_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, args, .. } = info;
    let Some(result_type) = &args.division_result else {
        return TokenStream::new();
    };
    let widen = if exact_in_f64(inner_type) {
        quote! { f64::from }
    } else {
        quote! { ::domain_types::traits::ApproxInto::approx_into }
    };
    quote! {
        #[automatically_derived]
        impl std::ops::Div<#inner_type> for #name {
            type Output = <#result_type as ::domain_types::traits::DivisionResult>::Output;

            fn div(self, rhs: #inner_type) -> Self::Output {
                let dividend: f64 = #widen(self.0);
                let divisor: f64 = #widen(rhs);
                <#result_type as ::domain_types::traits::DivisionResult>::from_division(dividend / divisor)
            }
        }

        #[automatically_derived]
        impl std::ops::Div for #name {
            type Output = <#result_type as ::domain_types::traits::DivisionResult>::Output;

            fn div(self, rhs: Self) -> Self::Output {
                self / rhs.0
            }
        }
    }
}

/// Makes a float domain type usable as the target of `division_result(...)` on integer types.
fn generate_division_result_impl(info: &TypeInfo, validated: bool) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    // An `f64` result needs no conversion at all; only a narrower float loses anything.
    let narrow = if exact_in_f64(inner_type) {
        quote! { ::domain_types::traits::ApproxInto::approx_into(value) }
    } else {
        quote! { value }
    };
    if validated {
        quote! {
            #[automatically_derived]
            impl ::domain_types::traits::DivisionResult for #name {
                type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

                fn from_division(value: f64) -> Self::Output {
                    Self::new(#narrow)
                }
            }
        }
    } else {
        // TODO: unvalidated targets accept `inf`/`NaN` from a division by zero silently;
        //       only validated float types (whose range validators reject them) catch that case.
        quote! {
            #[automatically_derived]
            impl ::domain_types::traits::DivisionResult for #name {
                type Output = Self;

                fn from_division(value: f64) -> Self::Output {
                    Self(#narrow)
                }
            }
        }
    }
}

/// The marker traits (`domain_types::traits`) identifying what shape of number a type is,
/// consumed generically elsewhere in the codebase (e.g. bounds on functions accepting "any
/// domain integer"). See that module for what each trait promises.
fn generate_domain_number_marker_impls(info: &TypeInfo, kind: &NumberKind) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;

    let value_trait = match kind.primitive {
        PrimitiveKind::Integer(_) => quote! { ::domain_types::traits::DomainIntegerValue<#inner_type> },
        PrimitiveKind::Float => quote! { ::domain_types::traits::DomainFloatValue<#inner_type> },
    };
    let value_marker = quote! {
        #[automatically_derived]
        impl #value_trait for #name {}
    };

    if !kind.is_number {
        return value_marker;
    }

    // Unvalidated number kinds additionally implement the primitive-agnostic `DomainNumber`
    // marker plus their primitive-specific `Domain{Integer,Float}Number`; validated kinds
    // implement `ValidatedDomain{Integer,Float}Number` instead (which carries the fallible
    // constructor contract `DomainNumber` doesn't) and skip the plain markers entirely.
    let number_markers = match (kind.primitive, kind.validated) {
        (PrimitiveKind::Integer(_), false) => quote! {
            #[automatically_derived]
            impl ::domain_types::traits::DomainNumber<#inner_type> for #name {}
            #[automatically_derived]
            impl ::domain_types::traits::DomainIntegerNumber<#inner_type> for #name {}
        },
        (PrimitiveKind::Integer(_), true) => quote! {
            #[automatically_derived]
            impl ::domain_types::traits::ValidatedDomainIntegerNumber<#inner_type> for #name {}
        },
        (PrimitiveKind::Float, false) => quote! {
            #[automatically_derived]
            impl ::domain_types::traits::DomainNumber<#inner_type> for #name {}
            #[automatically_derived]
            impl ::domain_types::traits::DomainFloatNumber<#inner_type> for #name {}
        },
        (PrimitiveKind::Float, true) => quote! {
            #[automatically_derived]
            impl ::domain_types::traits::ValidatedDomainFloatNumber<#inner_type> for #name {}
        },
    };

    quote! { #value_marker #number_markers }
}

/// A validated string type, checked either while the code is compiled or when the value arrives.
///
/// The validator takes a `&str`, not a `&String`: a `String` cannot exist in a `const` context, so
/// only the borrowed form can be checked there. One validator then serves both paths — the literals
/// written in the source and the values coming from the database, the environment and Telegram.
///
/// There is no `const` on `from_literal` here, and there cannot be: that is where the allocation
/// happens. Only the check is forced early, which is the half that matters.
fn generate_validated_domain_string_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, args, .. } = info;
    let validator = args.validator.as_ref().expect("a validator must be given");
    let error_msg = args.error_msg.as_ref().expect("an error message must be given");
    quote! {
        impl #name {
            pub fn new(value: String) -> Result<Self, ::domain_types::errors::DomainAssertionError<String>> {
                if #validator(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err(::domain_types::errors::DomainAssertionError::new(
                        value,
                        ::std::borrow::Cow::from(concat!(stringify!(#name), ' ', #error_msg))
                    ))
                }
            }

            /// Validates and hands the literal straight back. `literal!` calls this inside a
            /// `const` block, which is what makes a value the type would refuse fail the build.
            pub const fn check_literal(value: &'static str) -> &'static str {
                assert!(#validator(value), #error_msg);
                value
            }

            /// Wraps what [`Self::check_literal`] approved. Never call it directly — on its own it
            /// skips the check entirely, which is why `clippy.toml` forbids it.
            pub fn from_literal(value: &'static str) -> Self {
                Self(value.to_owned())
            }
        }

        // Hand-written instead of derived so deserialization can't bypass the validator.
        #[automatically_derived]
        impl<'de> ::serde::Deserialize<'de> for #name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let value = <String as ::serde::Deserialize>::deserialize(deserializer)?;
                Self::new(value).map_err(::serde::de::Error::custom)
            }
        }
    }
}

fn generate_domain_string_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, variant, .. } = info;
    let of = if matches!(variant, DomainTypeKind::String { validated: true }) {
        quote! {
            pub fn of(value: impl ToString) -> Result<Self, ::domain_types::errors::DomainAssertionError<String>> {
                Self::new(value.to_string())
            }
        }
    } else {
        quote! {
            pub fn of(value: impl ToString) -> Self {
                Self::new(value.to_string())
            }
        }
    };
    quote! {
        impl #name {
            #of

            pub fn value(&self) -> &str {
                self.0.as_str()
            }
        }

        impl AsRef<String> for #name {
            fn as_ref(&self) -> &String {
                &self.0
            }
        }

        #[automatically_derived]
        impl std::ops::Deref for #name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                self.0.as_str()
            }
        }

        #[automatically_derived]
        impl ::domain_types::traits::DomainString for #name {
            fn value(&self) -> &str {
                &self
            }
        }
    }
}

fn determine_inner_type_kind(ty: &Type) -> InnerTypeKind {
    if let Type::Group(group) = ty {
        return determine_inner_type_kind(&group.elem);
    }
    if let Type::Path(type_path) = ty {
        let signed_integer_types = ["i8", "i16", "i32", "i64"]
            .map(|ty| (ty, InnerTypeKind::Integer(IntegerSignedness::Signed)));
        let unsigned_integer_types = ["u8", "u16", "u32", "u64"]
            .map(|ty| (ty, InnerTypeKind::Integer(IntegerSignedness::Unsigned)));
        let float_types = ["f32", "f64"]
            .map(|ty| (ty, InnerTypeKind::Float));
        let string_types = [("String", InnerTypeKind::String)];
        let mapping = [].into_iter()
            .chain(signed_integer_types)
            .chain(unsigned_integer_types)
            .chain(float_types)
            .chain(string_types);
        if let Some(PathSegment { ident, .. }) = type_path.path.segments.last() {
            for (ty, response) in mapping {
                if ident == ty {
                    return response
                }
            }
            InnerTypeKind::Unsupported
        } else {
            InnerTypeKind::Unsupported
        }
    } else {
        InnerTypeKind::Unsupported
    }
}

fn generate_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;

    let is_validated = matches!(&info.variant,
        DomainTypeKind::Number(NumberKind { validated: true, .. }) | DomainTypeKind::String { validated: true });
    // An inherent constructor, so that call sites don't have to import the traits.
    // Validated types get their own inherent `new` returning a Result instead
    // (generated along with the other validated impls); it shadows the infallible trait method.
    let inherent_constructor = if is_validated {
        TokenStream::new()
    } else {
        match &info.variant {
            DomainTypeKind::String { .. } => quote! {
                impl #name {
                    pub fn new(value: #inner_type) -> Self {
                        Self(value)
                    }
                }
            },
            // No `literal` here. It exists to force a validator to run while the code is compiled,
            // and this type has none — `new` is `const` and infallible, so it is already everything
            // a constant needs.
            _ => quote! {
                impl #name {
                    pub const fn new(value: #inner_type) -> Self {
                        Self(value)
                    }
                }
            },
        }
    };
    let domain_type_impl = quote! {
        #inherent_constructor

        #[automatically_derived]
        impl ::domain_types::traits::DomainType<#inner_type> for #name {
            fn new(value: #inner_type) -> Self {
                Self(value)
            }
        }

        // Note: there is deliberately no `From<#inner_type> for #name` — it would allow
        // constructing validated types while bypassing the validator. Database decoding
        // goes through sqlx's `Type` derive (`#[sqlx(transparent)]`) with per-column
        // type overrides (`SELECT col AS "col: DomainType"`) in the queries instead.
        #[automatically_derived]
        impl ::std::convert::From<#name> for #inner_type {
            fn from(value: #name) -> Self {
                value.0
            }
        }
    };

    let DomainTypeKind::Number(kind) = &info.variant else {
        let domain_string_impls = generate_domain_string_impls(info);
        let validated_impls = if is_validated {
            generate_validated_domain_string_impls(info)
        } else {
            TokenStream::new()
        };
        return quote! {
            #domain_type_impl
            #domain_string_impls
            #validated_impls
        };
    };

    let mut pieces = vec![
        domain_type_impl,
        generate_domain_value_impls(info),
        generate_exact_widening_impls(info),
        generate_lossy_conversion_impls(info),
    ];

    if kind.validated {
        pieces.push(generate_validated_domain_number_impls(info));
    }
    if kind.is_number {
        pieces.push(match kind.primitive {
            PrimitiveKind::Integer(_) => generate_domain_integer_number_impls(info, kind.validated),
            PrimitiveKind::Float => generate_domain_float_number_impls(info, kind.validated),
        });
    }
    if kind.is_number && matches!(kind.primitive, PrimitiveKind::Integer(_)) {
        pieces.push(generate_division_operator_impls(info));
    }
    if matches!(kind.primitive, PrimitiveKind::Float) {
        pieces.push(generate_division_result_impl(info, kind.validated));
    }
    pieces.push(generate_domain_number_marker_impls(info, kind));

    quote! { #(#pieces)* }
}
