extern crate proc_macro;

use proc_macro2::TokenStream;
use quote::quote;
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

#[derive(PartialEq, Eq)]
enum IntegerSignedness {
    Signed,
    Unsigned
}

#[derive(PartialEq, Eq)]
enum NumberKind {
    IntegerValue(IntegerSignedness),
    IntegerNumber(IntegerSignedness),
    ValidatedIntegerValue(IntegerSignedness),
    ValidatedIntegerNumber(IntegerSignedness),
    FloatValue,
    FloatNumber,
    ValidatedFloatNumber,
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
                let content;
                syn::parenthesized!(content in input);

                // Parse validator expression
                validator = Some(content.parse()?);

                // Parse comma
                content.parse::<syn::Token![,]>()?;

                // Parse error_message
                content.parse::<kw::error_message>()?;
                let msg_content;
                syn::parenthesized!(msg_content in content);
                error_msg = Some(msg_content.parse()?);
            }
            else if lookahead.peek(kw::division_result) {
                input.parse::<kw::division_result>()?;
                let content;
                syn::parenthesized!(content in input);
                division_result = Some(content.parse()?);
            }
            else if lookahead.peek(kw::features) {
                input.parse::<kw::features>()?;
                let content;
                syn::parenthesized!(content in input);

                // Parse a comma-separated list of feature flags in any order
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
        InnerTypeKind::Integer(signedness) if args.number && args.validator.is_some() => DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(signedness)),
        InnerTypeKind::Integer(signedness) if args.validator.is_some() => DomainTypeKind::Number(NumberKind::ValidatedIntegerValue(signedness)),
        InnerTypeKind::Integer(signedness) if args.number => DomainTypeKind::Number(NumberKind::IntegerNumber(signedness)),
        InnerTypeKind::Integer(signedness) => DomainTypeKind::Number(NumberKind::IntegerValue(signedness)),
        InnerTypeKind::Float if args.number && args.validator.is_some() => DomainTypeKind::Number(NumberKind::ValidatedFloatNumber),
        InnerTypeKind::Float if args.number => DomainTypeKind::Number(NumberKind::FloatNumber),
        InnerTypeKind::Float => DomainTypeKind::Number(NumberKind::FloatValue),
        InnerTypeKind::String => DomainTypeKind::String,
        InnerTypeKind::Unsupported => panic!("unsupported domain type"),
    };

    if args.division_result.is_some() && !matches!(variant,
        DomainTypeKind::Number(NumberKind::IntegerNumber(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(_))
    ) {
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
    let domain_type_derives = vec![
        quote! { Clone },
        quote! { Debug },
        quote! { ::serde::Serialize },
    ];
    let domain_value_derives = vec![
        quote! { Default },
        quote! { PartialEq },
        quote! { PartialOrd },
    ];
    let eq_ord_hash_derives = vec![
        quote! { Eq },
        quote! { Ord },
        quote! { Hash },
    ];
    let copy_derive = vec![
        quote! { Copy },
    ];
    let neg_derive = vec![
        quote! { ::derive_more::Neg },
    ];
    
    let type_specific_derives = match &info.variant {
        DomainTypeKind::String =>
            [domain_value_derives, eq_ord_hash_derives].concat(),
        // Validated types never derive Neg: it would construct the negated value directly,
        // bypassing the validator (e.g. -Page(1) would produce an invalid Page(-1)).
        DomainTypeKind::Number(NumberKind::IntegerValue(IntegerSignedness::Unsigned)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerValue(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(_)) =>
            [domain_value_derives, eq_ord_hash_derives, copy_derive].concat(),
        DomainTypeKind::Number(NumberKind::IntegerValue(IntegerSignedness::Signed)) =>
            [domain_value_derives, eq_ord_hash_derives, copy_derive, neg_derive].concat(),
        DomainTypeKind::Number(NumberKind::IntegerNumber(IntegerSignedness::Unsigned)) => 
            [domain_value_derives, eq_ord_hash_derives, copy_derive].concat(),
        DomainTypeKind::Number(NumberKind::IntegerNumber(IntegerSignedness::Signed)) =>
            [domain_value_derives, eq_ord_hash_derives, copy_derive, neg_derive].concat(),
        DomainTypeKind::Number(NumberKind::FloatValue) | DomainTypeKind::Number(NumberKind::ValidatedFloatNumber) =>
            [domain_value_derives, copy_derive].concat(),
        // Arithmetic operators for float numbers are generated as explicit impls
        // (see generate_domain_float_number_impls), not derived: derive_more's op derives
        // don't produce the `Op<T>` / `Op<Self>` combination the DomainNumber trait requires.
        DomainTypeKind::Number(NumberKind::FloatNumber) =>
            [domain_value_derives, copy_derive].concat(),
    };
    
    let mut derives = [domain_type_derives, type_specific_derives].concat();
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
    let is_validated = matches!(variant,
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerValue(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedFloatNumber)
    );
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

fn generate_domain_number_impls(info: &TypeInfo) -> TokenStream {    
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        #[automatically_derived]
        impl ::domain_types::traits::DomainNumber<#inner_type> for #name {}
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

fn generate_domain_integer_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        impl #name {
            pub fn overflowing_add_primitive(self, rhs: #inner_type) -> (Self, bool) {
                let (new_value, is_overflow) = self.0.overflowing_add(rhs);
                (Self(new_value), is_overflow)
            }

            pub fn overflowing_add(self, rhs: Self) -> (Self, bool) {
                self.overflowing_add_primitive(rhs.0)
            }

            pub fn saturating_add_primitive(self, rhs: #inner_type) -> Self {
                Self(self.0.saturating_add(rhs))
            }

            pub fn saturating_add(self, rhs: Self) -> Self {
                self.saturating_add_primitive(rhs.0)
            }

            pub fn overflowing_sub_primitive(self, rhs: #inner_type) -> (Self, bool) {
                let (new_value, is_overflow) = self.0.overflowing_sub(rhs);
                (Self(new_value), is_overflow)
            }

            pub fn overflowing_sub(self, rhs: Self) -> (Self, bool) {
                self.overflowing_sub_primitive(rhs.0)
            }

            pub fn saturating_sub_primitive(self, rhs: #inner_type) -> Self {
                Self(self.0.saturating_sub(rhs))
            }

            pub fn saturating_sub(self, rhs: Self) -> Self {
                self.saturating_sub_primitive(rhs.0)
            }

            pub fn overflowing_mul_primitive(self, rhs: #inner_type) -> (Self, bool) {
                let (new_value, is_overflow) = self.0.overflowing_mul(rhs);
                (Self(new_value), is_overflow)
            }

            pub fn overflowing_mul(self, rhs: Self) -> (Self, bool) {
                self.overflowing_mul_primitive(rhs.0)
            }

            pub fn saturating_mul_primitive(self, rhs: #inner_type) -> Self {
                Self(self.0.saturating_mul(rhs))
            }

            pub fn saturating_mul(self, rhs: Self) -> Self {
                self.saturating_mul_primitive(rhs.0)
            }

            /// Integer division producing `Self`. For a division producing a float domain type,
            /// annotate the type with `division_result(...)` and use the `/` operator instead.
            pub fn overflowing_div_primitive(self, rhs: #inner_type) -> (Self, bool) {
                let (new_value, is_overflow) = self.0.overflowing_div(rhs);
                (Self(new_value), is_overflow)
            }

            pub fn overflowing_div(self, rhs: Self) -> (Self, bool) {
                self.overflowing_div_primitive(rhs.0)
            }

            pub fn saturating_div_primitive(self, rhs: #inner_type) -> Self {
                Self(self.0.saturating_div(rhs))
            }

            pub fn saturating_div(self, rhs: Self) -> Self {
                self.saturating_div_primitive(rhs.0)
            }

            // No `saturating_rem`: std doesn't provide one either (remainder can only
            // overflow on `MIN % -1`, which `overflowing_rem` reports explicitly).
            pub fn overflowing_rem_primitive(self, rhs: #inner_type) -> (Self, bool) {
                let (new_value, is_overflow) = self.0.overflowing_rem(rhs);
                (Self(new_value), is_overflow)
            }

            pub fn overflowing_rem(self, rhs: Self) -> (Self, bool) {
                self.overflowing_rem_primitive(rhs.0)
            }
        }

        #[automatically_derived]
        impl std::ops::Add<#inner_type> for #name {
            type Output = Self;

            fn add(self, rhs: #inner_type) -> Self::Output {
                self.saturating_add_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub<#inner_type> for #name {
            type Output = Self;

            fn sub(self, rhs: #inner_type) -> Self::Output {
                self.saturating_sub_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul<#inner_type> for #name {
            type Output = Self;

            fn mul(self, rhs: #inner_type) -> Self::Output {
                self.saturating_mul_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Add for #name {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                self.saturating_add(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub for #name {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                self.saturating_sub(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul for #name {
            type Output = Self;

            fn mul(self, rhs: Self) -> Self::Output {
                self.saturating_mul(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::AddAssign<#inner_type> for #name {
            fn add_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_add(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::SubAssign<#inner_type> for #name {
            fn sub_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_sub(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::MulAssign<#inner_type> for #name {
            fn mul_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_mul(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::AddAssign for #name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_add(rhs.0);
            }
        }

        #[automatically_derived]
        impl std::ops::SubAssign for #name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_sub(rhs.0);
            }
        }

        #[automatically_derived]
        impl std::ops::MulAssign for #name {
            fn mul_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_mul(rhs.0);
            }
        }
    }
}

fn generate_validated_domain_integer_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        impl #name {
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

            pub fn overflowing_add_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Addition;
                Self::perform_arithmetic_operation(self.0, rhs, operation, #inner_type::overflowing_add)
            }

            pub fn overflowing_add(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.overflowing_add_primitive(rhs.0)
            }

            pub fn saturating_add_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_add(rhs))
            }

            pub fn saturating_add(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_add_primitive(rhs.0)
            }

            pub fn overflowing_sub_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Subtraction;
                Self::perform_arithmetic_operation(self.0, rhs, operation, #inner_type::overflowing_sub)
            }

            pub fn overflowing_sub(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.overflowing_sub_primitive(rhs.0)
            }

            pub fn saturating_sub_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_sub(rhs))
            }

            pub fn saturating_sub(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_sub_primitive(rhs.0)
            }

            pub fn overflowing_mul_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Multiplication;
                Self::perform_arithmetic_operation(self.0, rhs, operation, #inner_type::overflowing_mul)
            }

            pub fn overflowing_mul(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.overflowing_mul_primitive(rhs.0)
            }

            pub fn saturating_mul_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_mul(rhs))
            }

            pub fn saturating_mul(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_mul_primitive(rhs.0)
            }

            pub fn overflowing_div_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Division;
                Self::perform_arithmetic_operation(self.0, rhs, operation, #inner_type::overflowing_div)
            }

            pub fn overflowing_div(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.overflowing_div_primitive(rhs.0)
            }

            pub fn saturating_div_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_div(rhs))
            }

            pub fn saturating_div(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_div_primitive(rhs.0)
            }

            pub fn overflowing_rem_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Remainder;
                Self::perform_arithmetic_operation(self.0, rhs, operation, #inner_type::overflowing_rem)
            }

            pub fn overflowing_rem(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.overflowing_rem_primitive(rhs.0)
            }
        }

        #[automatically_derived]
        impl std::ops::Add<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn add(self, rhs: #inner_type) -> Self::Output {
                self.saturating_add_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn sub(self, rhs: #inner_type) -> Self::Output {
                self.saturating_sub_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn mul(self, rhs: #inner_type) -> Self::Output {
                self.saturating_mul_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Add for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn add(self, rhs: Self) -> Self::Output {
                self.saturating_add(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn sub(self, rhs: Self) -> Self::Output {
                self.saturating_sub(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn mul(self, rhs: Self) -> Self::Output {
                self.saturating_mul(rhs)
            }
        }
    }
}

fn generate_domain_float_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        #[automatically_derived]
        impl std::ops::Add<#inner_type> for #name {
            type Output = Self;

            fn add(self, rhs: #inner_type) -> Self::Output {
                Self(self.0 + rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub<#inner_type> for #name {
            type Output = Self;

            fn sub(self, rhs: #inner_type) -> Self::Output {
                Self(self.0 - rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul<#inner_type> for #name {
            type Output = Self;

            fn mul(self, rhs: #inner_type) -> Self::Output {
                Self(self.0 * rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Div<#inner_type> for #name {
            type Output = Self;

            fn div(self, rhs: #inner_type) -> Self::Output {
                Self(self.0 / rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem<#inner_type> for #name {
            type Output = Self;

            fn rem(self, rhs: #inner_type) -> Self::Output {
                Self(self.0 % rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Add for #name {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                self + rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Sub for #name {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                self - rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Mul for #name {
            type Output = Self;

            fn mul(self, rhs: Self) -> Self::Output {
                self * rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Div for #name {
            type Output = Self;

            fn div(self, rhs: Self) -> Self::Output {
                self / rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Rem for #name {
            type Output = Self;

            fn rem(self, rhs: Self) -> Self::Output {
                self % rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::AddAssign<#inner_type> for #name {
            fn add_assign(&mut self, rhs: #inner_type) {
                self.0 += rhs;
            }
        }

        #[automatically_derived]
        impl std::ops::SubAssign<#inner_type> for #name {
            fn sub_assign(&mut self, rhs: #inner_type) {
                self.0 -= rhs;
            }
        }

        #[automatically_derived]
        impl std::ops::MulAssign<#inner_type> for #name {
            fn mul_assign(&mut self, rhs: #inner_type) {
                self.0 *= rhs;
            }
        }

        #[automatically_derived]
        impl std::ops::DivAssign<#inner_type> for #name {
            fn div_assign(&mut self, rhs: #inner_type) {
                self.0 /= rhs;
            }
        }

        #[automatically_derived]
        impl std::ops::AddAssign for #name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        #[automatically_derived]
        impl std::ops::SubAssign for #name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        #[automatically_derived]
        impl std::ops::MulAssign for #name {
            fn mul_assign(&mut self, rhs: Self) {
                self.0 *= rhs.0;
            }
        }

        #[automatically_derived]
        impl std::ops::DivAssign for #name {
            fn div_assign(&mut self, rhs: Self) {
                self.0 /= rhs.0;
            }
        }
    }
}

fn generate_validated_domain_float_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        #[automatically_derived]
        impl std::ops::Add<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn add(self, rhs: #inner_type) -> Self::Output {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 + rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Sub<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn sub(self, rhs: #inner_type) -> Self::Output {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 - rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Mul<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn mul(self, rhs: #inner_type) -> Self::Output {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 * rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Div<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn div(self, rhs: #inner_type) -> Self::Output {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 / rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem<#inner_type> for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn rem(self, rhs: #inner_type) -> Self::Output {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0 % rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Add for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn add(self, rhs: Self) -> Self::Output {
                self + rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Sub for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn sub(self, rhs: Self) -> Self::Output {
                self - rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Mul for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn mul(self, rhs: Self) -> Self::Output {
                self * rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Div for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn div(self, rhs: Self) -> Self::Output {
                self / rhs.0
            }
        }

        #[automatically_derived]
        impl std::ops::Rem for #name {
            type Output = Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>>;

            fn rem(self, rhs: Self) -> Self::Output {
                self % rhs.0
            }
        }
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
    // An inherent constructor, so that call sites don't have to import the traits.
    // Validated types get their own inherent `new` returning a Result instead
    // (generated along with the other validated impls); it shadows the infallible trait method.
    let inherent_constructor = match &info.variant {
        DomainTypeKind::Number(NumberKind::ValidatedIntegerValue(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(_)) |
        DomainTypeKind::Number(NumberKind::ValidatedFloatNumber) =>
            TokenStream::new(),
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
    match &info.variant {
        DomainTypeKind::Number(kind) => {
            let domain_value_impls = generate_domain_value_impls(info);
            match kind {
                NumberKind::IntegerValue(_) => {
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}
                    }
                }
                NumberKind::ValidatedIntegerValue(_) => {
                    let validated_domain_number_impls = generate_validated_domain_number_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #validated_domain_number_impls

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}
                    }
                }
                NumberKind::FloatValue => {
                    let division_result_impl = generate_division_result_impl(info, false);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #division_result_impl

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatValue<#inner_type> for #name {}
                    }
                }
                NumberKind::IntegerNumber(_) => {
                    let domain_number_impls = generate_domain_number_impls(info);
                    let domain_integer_number_impls = generate_domain_integer_number_impls(info);
                    let division_operator_impls = generate_division_operator_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #domain_number_impls
                        #domain_integer_number_impls
                        #division_operator_impls

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::ValidatedIntegerNumber(_) => {
                    let validated_domain_number_impls = generate_validated_domain_number_impls(info);
                    let validated_domain_integer_number_impls = generate_validated_domain_integer_number_impls(info);
                    let division_operator_impls = generate_division_operator_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #validated_domain_number_impls
                        #validated_domain_integer_number_impls
                        #division_operator_impls

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}

                        #[automatically_derived]
                        impl ::domain_types::traits::ValidatedDomainIntegerNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::FloatNumber => {
                    let domain_number_impls = generate_domain_number_impls(info);
                    let float_number_ops = generate_domain_float_number_impls(info);
                    let division_result_impl = generate_division_result_impl(info, false);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #domain_number_impls
                        #float_number_ops
                        #division_result_impl

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatValue<#inner_type> for #name {}
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::ValidatedFloatNumber => {
                    let validated_domain_number_impls = generate_validated_domain_number_impls(info);
                    let validated_domain_float_number_impls = generate_validated_domain_float_number_impls(info);
                    let division_result_impl = generate_division_result_impl(info, true);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #validated_domain_number_impls
                        #validated_domain_float_number_impls
                        #division_result_impl

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatValue<#inner_type> for #name {}

                        #[automatically_derived]
                        impl ::domain_types::traits::ValidatedDomainFloatNumber<#inner_type> for #name {}
                    }
                }
            }
        }
        DomainTypeKind::String => {
            let domain_string_impls = generate_domain_string_impls(info);
            quote! {
                #domain_type_impl
                #domain_string_impls
            }
        }
    }
}
