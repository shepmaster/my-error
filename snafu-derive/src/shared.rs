pub(crate) use self::context_module::ContextModule;
pub(crate) use self::context_selector::ContextSelector;
pub(crate) use self::display::{Display, DisplayMatchArm};
pub(crate) use self::error::{Error, ErrorSourceMatchArm};
pub(crate) use self::error_compat::{ErrorCompat, ErrorCompatBacktraceMatchArm};

pub mod context_module {
    use crate::ModuleName;
    use heck::SnakeCase;
    use proc_macro2::TokenStream;
    use quote::{quote, ToTokens};
    use syn::Ident;

    #[derive(Copy, Clone)]
    pub(crate) struct ContextModule<'a, T> {
        pub container_name: &'a Ident,
        pub module_name: &'a ModuleName,
        pub visibility: Option<&'a dyn ToTokens>,
        pub body: &'a T,
    }

    impl<'a, T> ToTokens for ContextModule<'a, T>
    where
        T: ToTokens,
    {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let module_name = match self.module_name {
                ModuleName::Default => {
                    let name_str = self.container_name.to_string().to_snake_case();
                    syn::Ident::new(&name_str, self.container_name.span())
                }
                ModuleName::Custom(name) => name.clone(),
            };

            let visibility = self.visibility;
            let body = self.body;

            let module_tokens = quote! {
                #visibility mod #module_name {
                    use super::*;

                    #body
                }
            };

            stream.extend(module_tokens);
        }
    }
}

pub mod context_selector {
    use crate::{ContextSelectorKind, Field, SuffixKind};
    use proc_macro2::TokenStream;
    use quote::{format_ident, quote, IdentFragment, ToTokens};

    const DEFAULT_SUFFIX: &str = "Snafu";

    #[derive(Copy, Clone)]
    pub(crate) struct ContextSelector<'a> {
        pub backtrace_field: Option<&'a Field>,
        pub crate_root: &'a dyn ToTokens,
        pub error_constructor_name: &'a dyn ToTokens,
        pub original_generics_without_defaults: &'a [TokenStream],
        pub parameterized_error_name: &'a dyn ToTokens,
        pub selector_doc_string: &'a str,
        pub selector_kind: &'a ContextSelectorKind,
        pub selector_name: &'a proc_macro2::Ident,
        pub user_fields: &'a [Field],
        pub visibility: Option<&'a dyn ToTokens>,
        pub where_clauses: &'a [TokenStream],
    }

    impl ToTokens for ContextSelector<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            use self::ContextSelectorKind::*;

            let context_selector = match self.selector_kind {
                Context { source_field, .. } => {
                    let context_selector_type = self.generate_type();
                    let context_selector_impl = match source_field {
                        Some(_) => None,
                        None => Some(self.generate_leaf()),
                    };
                    let context_selector_into_error_impl =
                        self.generate_into_error(source_field.as_ref());

                    quote! {
                        #context_selector_type
                        #context_selector_impl
                        #context_selector_into_error_impl
                    }
                }
                Whatever {
                    source_field,
                    message_field,
                } => self.generate_whatever(source_field.as_ref(), message_field),
                NoContext { source_field } => self.generate_from_source(source_field),
            };

            stream.extend(context_selector)
        }
    }

    impl ContextSelector<'_> {
        fn user_field_generics(&self) -> Vec<proc_macro2::Ident> {
            (0..self.user_fields.len())
                .map(|i| format_ident!("__T{}", i))
                .collect()
        }

        fn user_field_names(&self) -> Vec<&syn::Ident> {
            self.user_fields
                .iter()
                .map(|Field { name, .. }| name)
                .collect()
        }

        fn parameterized_selector_name(&self) -> TokenStream {
            let selector_name = self.selector_name.to_string();
            let selector_name = selector_name.trim_end_matches("Error");
            let suffix: &dyn IdentFragment = match self.selector_kind {
                ContextSelectorKind::Context {
                    suffix: SuffixKind::Some(suffix),
                    ..
                } => suffix,
                ContextSelectorKind::Context {
                    suffix: SuffixKind::None,
                    ..
                } => &"",
                _ => &DEFAULT_SUFFIX,
            };
            let selector_name = format_ident!(
                "{}{}",
                selector_name,
                suffix,
                span = self.selector_name.span()
            );
            let user_generics = self.user_field_generics();

            quote! { #selector_name<#(#user_generics,)*> }
        }

        fn extended_where_clauses(&self) -> Vec<TokenStream> {
            let user_fields = self.user_fields;
            let user_field_generics = self.user_field_generics();
            let where_clauses = self.where_clauses;

            let target_types = user_fields
                .iter()
                .map(|Field { ty, .. }| quote! { ::core::convert::Into<#ty>});

            user_field_generics
                .into_iter()
                .zip(target_types)
                .map(|(gen, bound)| quote! { #gen: #bound })
                .chain(where_clauses.iter().cloned())
                .collect()
        }

        fn transfer_user_fields(&self) -> Vec<TokenStream> {
            self.user_field_names()
                .into_iter()
                .map(|name| {
                    quote! { #name: ::core::convert::Into::into(self.#name) }
                })
                .collect()
        }

        fn construct_backtrace_field(&self) -> Option<TokenStream> {
            self.backtrace_field.map(|field| {
                let crate_root = self.crate_root;
                let name = &field.name;
                quote! { #name: #crate_root::GenerateBacktrace::generate(), }
            })
        }

        fn generate_type(self) -> TokenStream {
            let visibility = self.visibility;
            let parameterized_selector_name = self.parameterized_selector_name();
            let user_field_generics = self.user_field_generics();
            let user_field_names = self.user_field_names();
            let selector_doc_string = self.selector_doc_string;

            let body = if user_field_names.is_empty() {
                quote! { ; }
            } else {
                quote! {
                    {
                        #(
                            #[allow(missing_docs)]
                            #visibility #user_field_names: #user_field_generics
                        ),*
                    }
                }
            };

            quote! {
                #[derive(Debug, Copy, Clone)]
                #[doc = #selector_doc_string]
                #visibility struct #parameterized_selector_name #body
            }
        }

        fn generate_leaf(self) -> TokenStream {
            let error_constructor_name = self.error_constructor_name;
            let original_generics_without_defaults = self.original_generics_without_defaults;
            let parameterized_error_name = self.parameterized_error_name;
            let parameterized_selector_name = self.parameterized_selector_name();
            let user_field_generics = self.user_field_generics();
            let visibility = self.visibility;
            let extended_where_clauses = self.extended_where_clauses();
            let transfer_user_fields = self.transfer_user_fields();
            let construct_backtrace_field = self.construct_backtrace_field();

            quote! {
                impl<#(#user_field_generics,)*> #parameterized_selector_name {
                    #[doc = "Consume the selector and return the associated error"]
                    #[must_use]
                    #visibility fn build<#(#original_generics_without_defaults,)*>(self) -> #parameterized_error_name
                    where
                        #(#extended_where_clauses),*
                    {
                        #error_constructor_name {
                            #construct_backtrace_field
                            #(#transfer_user_fields,)*
                        }
                    }

                    #[doc = "Consume the selector and return a `Result` with the associated error"]
                    #visibility fn fail<#(#original_generics_without_defaults,)* __T>(self) -> ::core::result::Result<__T, #parameterized_error_name>
                    where
                        #(#extended_where_clauses),*
                    {
                        ::core::result::Result::Err(self.build())
                    }
                }
            }
        }

        fn generate_into_error(self, source_field: Option<&crate::SourceField>) -> TokenStream {
            let crate_root = self.crate_root;
            let error_constructor_name = self.error_constructor_name;
            let original_generics_without_defaults = self.original_generics_without_defaults;
            let parameterized_error_name = self.parameterized_error_name;
            let parameterized_selector_name = self.parameterized_selector_name();
            let user_field_generics = self.user_field_generics();
            let extended_where_clauses = self.extended_where_clauses();
            let transfer_user_fields = self.transfer_user_fields();
            let construct_backtrace_field = self.construct_backtrace_field();

            let (source_ty, transfer_source_field) = match source_field {
                Some(source_field) => {
                    let (ty, transfer) = build_source_info(source_field);
                    (quote! { #ty }, transfer)
                }
                None => (quote! { #crate_root::NoneError }, quote! {}),
            };

            quote! {
                impl<#(#original_generics_without_defaults,)* #(#user_field_generics,)*> #crate_root::IntoError<#parameterized_error_name> for #parameterized_selector_name
                where
                    #parameterized_error_name: #crate_root::Error + #crate_root::ErrorCompat,
                    #(#extended_where_clauses),*
                {
                    type Source = #source_ty;

                    fn into_error(self, error: Self::Source) -> #parameterized_error_name {
                        #error_constructor_name {
                            #transfer_source_field
                            #construct_backtrace_field
                            #(#transfer_user_fields),*
                        }
                    }
                }
            }
        }

        fn generate_whatever(
            self,
            source_field: Option<&crate::SourceField>,
            message_field: &crate::Field,
        ) -> TokenStream {
            let crate_root = self.crate_root;
            let parameterized_error_name = self.parameterized_error_name;
            let error_constructor_name = self.error_constructor_name;
            let construct_backtrace_field = self.construct_backtrace_field();

            // testme: transform

            let (source_ty, transfer_source_field, empty_source_field) = match source_field {
                Some(f) => {
                    let source_field_type = f.transformation.ty();
                    let source_field_name = &f.name;
                    let source_transformation = f.transformation.transformation();

                    (
                        quote! { #source_field_type },
                        Some(quote! { #source_field_name: (#source_transformation)(error), }),
                        Some(quote! { #source_field_name: core::option::Option::None, }),
                    )
                }
                None => (quote! { #crate_root::NoneError }, None, None),
            };

            let message_field_name = &message_field.name;

            quote! {
                impl #crate_root::FromString for #parameterized_error_name {
                    type Source = #source_ty;

                    fn without_source(message: String) -> Self {
                        #error_constructor_name {
                            #empty_source_field
                            #message_field_name: message,
                            #construct_backtrace_field
                        }
                    }

                    fn with_source(error: Self::Source, message: String) -> Self {
                        #error_constructor_name {
                            #transfer_source_field
                            #message_field_name: message,
                            #construct_backtrace_field
                        }
                    }
                }
            }
        }

        fn generate_from_source(self, source_field: &crate::SourceField) -> TokenStream {
            let parameterized_error_name = self.parameterized_error_name;
            let error_constructor_name = self.error_constructor_name;
            let construct_backtrace_field = self.construct_backtrace_field();
            let original_generics_without_defaults = self.original_generics_without_defaults;
            let user_field_generics = self.user_field_generics();
            let where_clauses = self.where_clauses;

            let (source_field_type, transfer_source_field) = build_source_info(source_field);

            quote! {
                impl<#(#original_generics_without_defaults,)* #(#user_field_generics,)*> ::core::convert::From<#source_field_type> for #parameterized_error_name
                where
                    #(#where_clauses),*
                {
                    fn from(error: #source_field_type) -> Self {
                        #error_constructor_name {
                            #transfer_source_field
                            #construct_backtrace_field
                        }
                    }
                }
            }
        }
    }

    // Assumes that the error is in a variable called "error"
    fn build_source_info(source_field: &crate::SourceField) -> (&syn::Type, TokenStream) {
        let source_field_name = source_field.name();
        let source_field_type = source_field.transformation.ty();
        let source_transformation = source_field.transformation.transformation();

        (
            source_field_type,
            quote! { #source_field_name: (#source_transformation)(error), },
        )
    }
}

pub mod display {
    use crate::{Field, SourceField};
    use proc_macro2::TokenStream;
    use quote::{quote, ToTokens};

    struct StaticIdent(&'static str);

    impl quote::ToTokens for StaticIdent {
        fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
            proc_macro2::Ident::new(self.0, proc_macro2::Span::call_site()).to_tokens(tokens)
        }
    }

    const FORMATTER_ARG: StaticIdent = StaticIdent("__snafu_display_formatter");

    pub(crate) struct Display<'a> {
        pub(crate) arms: &'a [TokenStream],
        pub(crate) original_generics: &'a [TokenStream],
        pub(crate) parameterized_error_name: &'a dyn ToTokens,
        pub(crate) where_clauses: &'a [TokenStream],
    }

    impl ToTokens for Display<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                arms,
                original_generics,
                parameterized_error_name,
                where_clauses,
            } = *self;

            let display_impl = quote! {
                #[allow(single_use_lifetimes)]
                impl<#(#original_generics),*> ::core::fmt::Display for #parameterized_error_name
                where
                    #(#where_clauses),*
                {
                    fn fmt(&self, #FORMATTER_ARG: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                        #[allow(unused_variables)]
                        match *self {
                            #(#arms),*
                        }
                    }
                }
            };

            stream.extend(display_impl);
        }
    }

    pub(crate) struct DisplayMatchArm<'a> {
        pub(crate) backtrace_field: Option<&'a crate::Field>,
        pub(crate) default_name: &'a dyn ToTokens,
        pub(crate) display_format: Option<&'a dyn ToTokens>,
        pub(crate) doc_comment: &'a str,
        pub(crate) pattern_ident: &'a dyn ToTokens,
        pub(crate) selector_kind: &'a crate::ContextSelectorKind,
    }

    impl ToTokens for DisplayMatchArm<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                backtrace_field,
                default_name,
                display_format,
                doc_comment,
                pattern_ident,
                selector_kind,
            } = *self;

            let user_fields = selector_kind.user_fields();
            let source_field = selector_kind.source_field();
            let message_field = selector_kind.message_field();

            let format = match (display_format, source_field) {
                (Some(v), _) => quote! { #v },
                (None, _) if !doc_comment.is_empty() => {
                    quote! { #doc_comment }
                }
                (None, Some(f)) => {
                    let field_name = &f.name;
                    quote! { concat!(stringify!(#default_name), ": {}"), #field_name }
                }
                (None, None) => quote! { stringify!(#default_name)},
            };

            let field_names = user_fields
                .iter()
                .chain(backtrace_field)
                .chain(message_field)
                .map(Field::name)
                .chain(source_field.map(SourceField::name));

            let field_names = quote! { #(ref #field_names),* };

            let match_arm = quote! {
                #pattern_ident { #field_names } => {
                    write!(#FORMATTER_ARG, #format)
                }
            };

            stream.extend(match_arm);
        }
    }
}

pub mod error {
    use crate::{FieldContainer, SourceField};
    use proc_macro2::TokenStream;
    use quote::{quote, ToTokens};

    pub(crate) struct Error<'a> {
        pub(crate) crate_root: &'a dyn ToTokens,
        pub(crate) parameterized_error_name: &'a dyn ToTokens,
        pub(crate) description_arms: &'a [TokenStream],
        pub(crate) source_arms: &'a [TokenStream],
        pub(crate) original_generics: &'a [TokenStream],
        pub(crate) where_clauses: &'a [TokenStream],
    }

    impl ToTokens for Error<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                crate_root,
                parameterized_error_name,
                description_arms,
                source_arms,
                original_generics,
                where_clauses,
            } = *self;

            let description_fn = quote! {
                fn description(&self) -> &str {
                    match *self {
                        #(#description_arms)*
                    }
                }
            };

            let source_body = quote! {
                use #crate_root::AsErrorSource;
                match *self {
                    #(#source_arms)*
                }
            };

            let cause_fn = quote! {
                fn cause(&self) -> ::core::option::Option<&dyn #crate_root::Error> {
                    #source_body
                }
            };

            let source_fn = quote! {
                fn source(&self) -> ::core::option::Option<&(dyn #crate_root::Error + 'static)> {
                    #source_body
                }
            };

            let std_backtrace_fn = if cfg!(feature = "unstable-backtraces-impl-std") {
                Some(quote! {
                    fn backtrace(&self) -> ::core::option::Option<&::std::backtrace::Backtrace> {
                        #crate_root::ErrorCompat::backtrace(self)
                    }
                })
            } else {
                None
            };

            let error = quote! {
                #[allow(single_use_lifetimes)]
                impl<#(#original_generics),*> #crate_root::Error for #parameterized_error_name
                where
                    Self: ::core::fmt::Debug + ::core::fmt::Display,
                    #(#where_clauses),*
                {
                    #description_fn
                    #cause_fn
                    #source_fn
                    #std_backtrace_fn
                }
            };

            stream.extend(error);
        }
    }

    pub(crate) struct ErrorSourceMatchArm<'a> {
        pub(crate) field_container: &'a FieldContainer,
        pub(crate) pattern_ident: &'a dyn ToTokens,
    }

    impl ToTokens for ErrorSourceMatchArm<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                field_container: FieldContainer { selector_kind, .. },
                pattern_ident,
            } = *self;

            let source_field = selector_kind.source_field();

            let arm = match source_field {
                Some(source_field) => {
                    let SourceField {
                        name: field_name, ..
                    } = source_field;

                    let convert_to_error_source = if selector_kind.is_whatever() {
                        quote! {
                            #field_name.as_ref().map(|e| e.as_error_source())
                        }
                    } else {
                        quote! {
                            ::core::option::Option::Some(#field_name.as_error_source())
                        }
                    };

                    quote! {
                        #pattern_ident { ref #field_name, .. } => {
                            #convert_to_error_source
                        }
                    }
                }
                None => {
                    quote! {
                        #pattern_ident { .. } => { ::core::option::Option::None }
                    }
                }
            };

            stream.extend(arm);
        }
    }
}

pub mod error_compat {
    use crate::{Field, FieldContainer, SourceField};
    use proc_macro2::TokenStream;
    use quote::{quote, ToTokens};

    pub(crate) struct ErrorCompat<'a> {
        pub(crate) crate_root: &'a dyn ToTokens,
        pub(crate) parameterized_error_name: &'a dyn ToTokens,
        pub(crate) backtrace_arms: &'a [TokenStream],
        pub(crate) original_generics: &'a [TokenStream],
        pub(crate) where_clauses: &'a [TokenStream],
    }

    impl ToTokens for ErrorCompat<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                crate_root,
                parameterized_error_name,
                backtrace_arms,
                original_generics,
                where_clauses,
            } = *self;

            let backtrace_fn = quote! {
                fn backtrace(&self) -> ::core::option::Option<&#crate_root::Backtrace> {
                    match *self {
                        #(#backtrace_arms),*
                    }
                }
            };

            let error_compat_impl = quote! {
                #[allow(single_use_lifetimes)]
                impl<#(#original_generics),*> #crate_root::ErrorCompat for #parameterized_error_name
                where
                    #(#where_clauses),*
                {
                    #backtrace_fn
                }
            };

            stream.extend(error_compat_impl);
        }
    }

    pub(crate) struct ErrorCompatBacktraceMatchArm<'a> {
        pub(crate) crate_root: &'a dyn ToTokens,
        pub(crate) field_container: &'a FieldContainer,
        pub(crate) pattern_ident: &'a dyn ToTokens,
    }

    impl ToTokens for ErrorCompatBacktraceMatchArm<'_> {
        fn to_tokens(&self, stream: &mut TokenStream) {
            let Self {
                crate_root,
                field_container:
                    FieldContainer {
                        backtrace_field,
                        selector_kind,
                        ..
                    },
                pattern_ident,
            } = *self;

            let match_arm = match (selector_kind.source_field(), backtrace_field) {
                (Some(source_field), _) if source_field.backtrace_delegate => {
                    let SourceField {
                        name: field_name, ..
                    } = source_field;
                    quote! {
                        #pattern_ident { ref #field_name, .. } => { #crate_root::ErrorCompat::backtrace(#field_name) }
                    }
                }
                (_, Some(backtrace_field)) => {
                    let Field {
                        name: field_name, ..
                    } = backtrace_field;
                    quote! {
                        #pattern_ident { ref #field_name, .. } => { #crate_root::GenerateBacktrace::as_backtrace(#field_name) }
                    }
                }
                _ => {
                    quote! {
                        #pattern_ident { .. } => { ::core::option::Option::None }
                    }
                }
            };

            stream.extend(match_arm);
        }
    }
}
