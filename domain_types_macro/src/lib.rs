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
    syn::custom_keyword!(not_database_type);
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
    String,
}

enum InnerTypeKind {
    Unsupported,
    Integer(IntegerSignedness),
    Float,
    String,
}

struct TypeInfo<'a> {
    name: &'a Ident,
    inner_type: Type,
    variant: DomainTypeKind,
    args: DomainTypeAttr,
}

struct DomainTypeAttr {
    number: bool,
    not_database_type: bool,
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
        let mut not_database_type = false;
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
                let (nd, na) = parse_features(input)?;
                not_database_type = nd;
                no_auto_display = na;
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
            not_database_type,
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

    Ok((validator, error_msg))
}

/// Parses the comma-separated, order-independent flag list inside `features(...)`.
fn parse_features(input: ParseStream) -> syn::Result<(bool, bool)> {
    let content;
    syn::parenthesized!(content in input);

    let mut not_database_type = false;
    let mut no_auto_display = false;
    while !content.is_empty() {
        let feature_lookahead = content.lookahead1();
        if feature_lookahead.peek(kw::not_database_type) {
            content.parse::<kw::not_database_type>()?;
            not_database_type = true;
        } else if feature_lookahead.peek(kw::no_auto_display) {
            content.parse::<kw::no_auto_display>()?;
            no_auto_display = true;
        } else {
            return Err(feature_lookahead.error());
        }
        if !content.is_empty() {
            content.parse::<syn::Token![,]>()?;
        }
    }
    Ok((not_database_type, no_auto_display))
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
        InnerTypeKind::String => DomainTypeKind::String,
        InnerTypeKind::Unsupported => panic!("unsupported domain type"),
    };

    let is_integer_number = matches!(&variant,
        DomainTypeKind::Number(NumberKind { primitive: PrimitiveKind::Integer(_), is_number: true, .. })
    );
    if args.division_result.is_some() && !is_integer_number {
        panic!("division_result is only applicable to integer domain numbers")
    }

    let info = TypeInfo {
        name, inner_type, args, variant,
    };
    let derives = generate_derives(&info);
    let impls = generate_impls(&info);

    let sqlx_transparent = if !info.args.not_database_type {
        quote! { #[sqlx(transparent)] }
    } else {
        quote! {}
    };

    let TypeInfo { name, inner_type, .. } = info;
    // Generate the final struct with a conditional 'sqlx' attribute
    let output = quote! {
        #[derive(#(#derives),*)]
        #sqlx_transparent
        pub struct #name(#inner_type);

        #impls
    };

    proc_macro::TokenStream::from(output)
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

    match &info.variant {
        DomainTypeKind::String => {
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

    if !info.args.not_database_type {
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

            pub const fn literal(value: #inner_type) -> Self {
                assert!(#validator(&value), #error_msg);
                Self(value)
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
// TODO: `self.0 as f64` loses precision for 64-bit integers above 2^53;
//       consider rejecting `division_result` on i64/u64 domain types at macro-expansion time.
fn generate_division_operator_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, args, .. } = info;
    let Some(result_type) = &args.division_result else {
        return TokenStream::new();
    };
    quote! {
        #[automatically_derived]
        impl std::ops::Div<#inner_type> for #name {
            type Output = <#result_type as ::domain_types::traits::DivisionResult>::Output;

            fn div(self, rhs: #inner_type) -> Self::Output {
                <#result_type as ::domain_types::traits::DivisionResult>::from_division(self.0 as f64 / rhs as f64)
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
    if validated {
        quote! {
            #[automatically_derived]
            impl ::domain_types::traits::DivisionResult for #name {
                type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

                fn from_division(value: f64) -> Self::Output {
                    Self::new(value as #inner_type)
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
                    Self(value as #inner_type)
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

fn generate_domain_string_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, .. } = info;
    quote! {
        impl #name {
            pub fn of(value: impl ToString) -> Self {
                Self::new(value.to_string())
            }

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

    let is_validated = matches!(&info.variant, DomainTypeKind::Number(NumberKind { validated: true, .. }));
    // An inherent constructor, so that call sites don't have to import the traits.
    // Validated types get their own inherent `new` returning a Result instead
    // (generated along with the other validated impls); it shadows the infallible trait method.
    let inherent_constructor = if is_validated {
        TokenStream::new()
    } else {
        match &info.variant {
            DomainTypeKind::String => quote! {
                impl #name {
                    pub fn new(value: #inner_type) -> Self {
                        Self(value)
                    }
                }
            },
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
        return quote! {
            #domain_type_impl
            #domain_string_impls
        };
    };

    let mut pieces = vec![domain_type_impl, generate_domain_value_impls(info)];

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
