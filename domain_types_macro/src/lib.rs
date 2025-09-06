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
    ValidatedIntegerNumber(IntegerSignedness),
    FloatValue,
    FloatNumber,
    ValidatedFloatNumber,
}

#[derive(PartialEq, Eq)]
enum DomainTypeKind {
    Value,
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
    input: ItemStruct,
}

struct DomainTypeAttr {
    number: bool,
    not_database_type: bool,
    validator: Option<syn::Expr>,
    error_msg: Option<syn::LitStr>,
}

impl Parse for DomainTypeAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut number = false;
        let mut validator = None;
        let mut error_msg = None;
        let mut not_database_type = false;

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
            else if lookahead.peek(kw::features) {
                input.parse::<kw::features>()?;
                let content;
                syn::parenthesized!(content in input);

                if content.parse::<Option<kw::not_database_type>>()?.is_some() {
                    not_database_type = true;
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
            validator,
            error_msg,
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
        InnerTypeKind::Integer(signedness) if args.number => DomainTypeKind::Number(NumberKind::IntegerNumber(signedness)),
        InnerTypeKind::Integer(signedness) => DomainTypeKind::Number(NumberKind::IntegerValue(signedness)),
        InnerTypeKind::Float if args.number && args.validator.is_some() => DomainTypeKind::Number(NumberKind::ValidatedFloatNumber),
        InnerTypeKind::Float if args.number => DomainTypeKind::Number(NumberKind::FloatNumber),
        InnerTypeKind::Float => DomainTypeKind::Number(NumberKind::FloatValue),
        InnerTypeKind::String => DomainTypeKind::String,
        InnerTypeKind::Unsupported => panic!("unsupported domain type"),
    };

    let info = TypeInfo {
        name, inner_type, args, variant,
        input: input.clone()
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
        quote! { ::derive_more::Display },
    ];
    let domain_value_derives = vec![
        quote! { Default },
        quote! { PartialEq },
        quote! { PartialOrd },
    ];
    let eq_ord_derives = vec![
        quote! { Eq },
        quote! { Ord },
    ];
    let copy_derive = vec![
        quote! { Copy },
    ];
    let number_derives = vec![
        quote! { ::derive_more::Add },
        quote! { ::derive_more::Sub },
        quote! { ::derive_more::AddAssign },
        quote! { ::derive_more::SubAssign },
        
    ];
    let neg_derive = vec![
        quote! { ::derive_more::Neg },
    ];
    
    let type_specific_derives = match &info.variant { 
        DomainTypeKind::Value =>
            Vec::default(),
        DomainTypeKind::String =>
            [domain_value_derives, eq_ord_derives].concat(),
        DomainTypeKind::Number(NumberKind::IntegerValue(IntegerSignedness::Unsigned)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(IntegerSignedness::Unsigned)) =>
            [domain_value_derives, eq_ord_derives, copy_derive].concat(),
        DomainTypeKind::Number(NumberKind::IntegerValue(IntegerSignedness::Signed)) |
        DomainTypeKind::Number(NumberKind::ValidatedIntegerNumber(IntegerSignedness::Signed)) =>
            [domain_value_derives, eq_ord_derives, copy_derive, neg_derive].concat(),
        DomainTypeKind::Number(NumberKind::IntegerNumber(IntegerSignedness::Unsigned)) => 
            [domain_value_derives, eq_ord_derives, copy_derive, number_derives].concat(),
        DomainTypeKind::Number(NumberKind::IntegerNumber(IntegerSignedness::Signed)) =>
            [domain_value_derives, eq_ord_derives, copy_derive, number_derives, neg_derive].concat(),
        DomainTypeKind::Number(NumberKind::FloatValue) | DomainTypeKind::Number(NumberKind::ValidatedFloatNumber) =>
            [domain_value_derives, copy_derive].concat(),
        DomainTypeKind::Number(NumberKind::FloatNumber) =>
            [domain_value_derives, copy_derive, number_derives].concat(),
    };
    
    let mut derives = [domain_type_derives, type_specific_derives].concat();
    if info.args.validator.is_none() {
        derives.push(quote! { ::derive_more::Constructor });
    }
    if !info.args.not_database_type {
        derives.push(quote! { ::sqlx::Type })
    }
    
    derives
}

fn generate_domain_value_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {        
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
        #[automatically_derived]
        impl ::domain_types::traits::ValidatedDomainNumber<#inner_type> for #name {
            fn new(value: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                if #validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(::domain_types::errors::DomainAssertionError::new(
                        value,
                        concat!(stringify!(#name), ' ', #error_msg)
                    ))
                }
            }
        }

        impl #name {
            pub const fn literal(value: #inner_type) -> Self {
                assert!(#validator(&value), #error_msg);
                Self(value)
            }
        }
    }
}

fn generate_domain_integer_number_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, inner_type, .. } = info;
    quote! {
        pub fn overflowing_add_primitive(self, rhs: #inner_type) -> (Self, bool) {
            let (new_value, is_overflow) = self.0.overflowing_add(rhs);
            (Self(new_value), is_overflow)
        }

        pub fn overflowing_add(self, rhs: Self) -> (Self, bool) {
            self.overflowing_add_primitive(rhs.0);
        }

        pub fn saturating_add_primitive(self, rhs: #inner_type) -> Self {
            Self(self.0.saturating_add(rhs))
        }

        pub fn saturating_add(self, rhs: Self) -> Self {
            self.saturating_add_primitive(rhs.0)
        }

        pub fn overflowing_sub_primitive(self, rhs: #inner_type) -> (Self, bool) {
            let (new_value, is_overflow) = self.0.overflowing_sub(rhs);
            (Self(new_value, is_overflow))
        }

        pub fn overflowing_sub(self, rhs: Self) -> (Self, bool) {
            self.overflowing_sub_primitive(rhs.0);
        }

        pub fn saturating_sub_primitive(self, rhs: #inner_type) -> Self {
            Self(self.0.saturating_sub(rhs))
        }

        pub fn saturating_sub(self, rhs: Self) -> Self {
            self.saturating_sub_primitive(rhs.0)
        }

        pub fn overflowing_mul_primitive(self, rhs: #inner_type) -> (Self, bool) {
            let (new_value, is_overflow) = self.0.overflowing_mul(rhs);
            (Self(new_value, is_overflow))
        }

        pub fn overflowing_mul(self, rhs: Self) -> (Self, bool) {
            self.overflowing_mul_primitive(rhs.0);
        }

        pub fn saturating_mul_primitive(self, rhs: #inner_type) -> Self {
            Self(self.0.saturating_mul(rhs))
        }

        pub fn saturating_mul(self, rhs: Self) -> Self {
            self.saturating_mul_primitive(rhs.0)
        }

        pub fn overflowing_div_primitive(self, rhs: #inner_type) -> (Self, bool) {
            let (new_value, is_overflow) = self.0.overflowing_div(rhs);
            (Self(new_value, is_overflow))
        }

        pub fn overflowing_div(self, rhs: Self) -> (Self, bool) {
            self.overflowing_div_primitive(rhs.0);
        }

        pub fn saturating_div_primitive(self, rhs: #inner_type) -> Self {
            Self(self.0.saturating_div(rhs))
        }

        pub fn saturating_div(self, rhs: Self) -> Self {
            self.saturating_div_primitive(rhs.0)
        }

        pub fn overflowing_rem_primitive(self, rhs: #inner_type) -> (Self, bool) {
            let (new_value, is_overflow) = self.0.overflowing_rem(rhs);
            (Self(new_value, is_overflow))
        }

        pub fn overflowing_rem(self, rhs: Self) -> (Self, bool) {
            self.overflowing_rem_primitive(rhs.0);
        }

        pub fn saturating_rem_primitive(self, rhs: #inner_type) -> Self {
            Self(self.0.saturating_rem(rhs))
        }

        pub fn saturating_rem(self, rhs: Self) -> Self {
            self.saturating_rem_primitive(rhs.0)
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
        impl std::ops::Div<#inner_type> for #name {
            type Output = Self;

            fn div(self, rhs: #inner_type) -> Self::Output {
                self.saturating_div_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem<#inner_type> for #name {
            type Output = Self;

            fn rem(self, rhs: #inner_type) -> Self::Output {
                self.saturating_rem_primitive(rhs)
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
        impl std::ops::Div for #name {
            type Output = Self;

            fn div(self, rhs: Self) -> Self::Output {
                self.saturating_mul(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem for #name {
            type Output = Self;

            fn rem(self, rhs: Self) -> Self::Output {
                self.saturating_rem(rhs)
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
        impl std::ops::DivAssign<#inner_type> for #name {
            fn div_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_div(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::RemAssign<#inner_type> for #name {
            fn rem_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_rem(rhs);
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

        #[automatically_derived]
        impl std::ops::DivAssign for #name {
            fn div_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_div(rhs.0);
            }
        }

        #[automatically_derived]
        impl std::ops::RemAssign for #name {
            fn rem_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_rem(rhs.0);
            }
        }
    }
}

fn generate_domain_validated_integer_number_impls(info: &TypeInfo) -> TokenStream {
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
                    Err(::domain_types::errors::DomainAssertionError::new(new_value, cause.to_string()))
                }
            }

            pub fn add_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Addition;
                Self::perform_arithmetic_operation(self.0, rhs, operation, OverflowingAdd::overflowing_add)
            }

            pub fn add(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.add_primitive(rhs.0)
            }

            pub fn saturating_add_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_add(rhs))
            }

            pub fn saturating_add(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_add_primitive(rhs.0)
            }

            pub fn sub_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Subtraction;
                Self::perform_arithmetic_operation(self.0, rhs, operation, OverflowingSub::overflowing_sub)
            }

            pub fn sub(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.sub_primitive(rhs.0)
            }

            pub fn saturating_sub_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_sub(rhs))
            }

            pub fn saturating_sub(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_sub_primitive(rhs.0)
            }

            pub fn mul_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Multiplication;
                Self::perform_arithmetic_operation(self.0, rhs, operation, OverflowingMul::overflowing_mul)
            }

            pub fn mul(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.mul_primitive(rhs.0)
            }

            pub fn saturating_mul_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_mul(rhs))
            }

            pub fn saturating_mul(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_mul_primitive(rhs.0)
            }

            pub fn div_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Division;
                Self::perform_arithmetic_operation(self.0, rhs, operation, OverflowingDiv::overflowing_div)
            }

            pub fn div(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.div_primitive(rhs.0)
            }

            pub fn saturating_div_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_div(rhs))
            }

            pub fn saturating_div(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_div_primitive(rhs.0)
            }

            pub fn rem_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                let operation = ::domain_types::errors::ArithmeticOperation::Remainder;
                Self::perform_arithmetic_operation(self.0, rhs, operation, OverflowingRem::overflowing_rem)
            }

            pub fn rem(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.rem_primitive(rhs.0)
            }

            pub fn saturating_rem_primitive(self, rhs: #inner_type) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                <Self as ::domain_types::traits::ValidatedDomainNumber<#inner_type>>::new(self.0.saturating_rem(rhs))
            }

            pub fn saturating_rem(self, rhs: Self) -> Result<Self, ::domain_types::errors::DomainAssertionError<#inner_type>> {
                self.saturating_rem_primitive(rhs.0)
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
        impl std::ops::Div<#inner_type> for #name {
            type Output = Self;

            fn div(self, rhs: #inner_type) -> Self::Output {
                self.saturating_div_primitive(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem<#inner_type> for #name {
            type Output = Self;

            fn rem(self, rhs: #inner_type) -> Self::Output {
                self.saturating_rem_primitive(rhs)
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
        impl std::ops::Div for #name {
            type Output = Self;

            fn div(self, rhs: Self) -> Self::Output {
                self.saturating_mul(rhs)
            }
        }

        #[automatically_derived]
        impl std::ops::Rem for #name {
            type Output = Self;

            fn rem(self, rhs: Self) -> Self::Output {
                self.saturating_rem(rhs)
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
        impl std::ops::DivAssign<#inner_type> for #name {
            fn div_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_div(rhs);
            }
        }

        #[automatically_derived]
        impl std::ops::RemAssign<#inner_type> for #name {
            fn rem_assign(&mut self, rhs: #inner_type) {
                self.0 = self.0.saturating_rem(rhs);
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

        #[automatically_derived]
        impl std::ops::DivAssign for #name {
            fn div_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_div(rhs.0);
            }
        }

        #[automatically_derived]
        impl std::ops::RemAssign for #name {
            fn rem_assign(&mut self, rhs: Self) {
                self.0 = self.0.saturating_rem(rhs.0);
            }
        }
    }
}

fn generate_domain_string_impls(info: &TypeInfo) -> TokenStream {
    let TypeInfo { name, .. } = info;
    quote! {
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
    let domain_type_impl = quote! {
        #[automatically_derived]
        impl ::domain_types::traits::DomainType<#inner_type> for #name {}
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
                NumberKind::FloatValue => {
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatValue<#inner_type> for #name {}
                    }
                }
                NumberKind::IntegerNumber(_) => {
                    let domain_number_impls = generate_domain_number_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #domain_number_impls
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::ValidatedIntegerNumber(_) => {
                    let validated_domain_number_impls = generate_validated_domain_number_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #validated_domain_number_impls

                        #[automatically_derived]
                        impl ::domain_types::traits::DomainIntegerValue<#inner_type> for #name {}

                        #[automatically_derived]
                        impl ::domain_types::traits::ValidatedDomainIntegerNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::FloatNumber => {
                    let domain_number_impls = generate_domain_number_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #domain_number_impls
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatValue<#inner_type> for #name {}
                        
                        #[automatically_derived]
                        impl ::domain_types::traits::DomainFloatNumber<#inner_type> for #name {}
                    }
                }
                NumberKind::ValidatedFloatNumber => {
                    let validated_domain_number_impls = generate_validated_domain_number_impls(info);
                    quote! {
                        #domain_type_impl
                        #domain_value_impls
                        #validated_domain_number_impls

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
        _ => domain_type_impl
    }
}
