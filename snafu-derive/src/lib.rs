#![recursion_limit = "128"] // https://github.com/rust-lang/rust/issues/62059

extern crate proc_macro;

use crate::parse::attributes_from_syn;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::VecDeque;
use std::fmt;

mod parse;
mod shared;

// The snafu crate re-exports this and adds useful documentation.
#[proc_macro_derive(Snafu, attributes(snafu))]
pub fn snafu_derive(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).expect("Could not parse type to derive Error for");

    impl_snafu_macro(ast)
}

type MultiSynResult<T> = std::result::Result<T, Vec<syn::Error>>;

/// Some arbitrary tokens we treat as a black box
type UserInput = Box<dyn quote::ToTokens>;

enum ModuleName {
    Default,
    Custom(syn::Ident),
}

enum SnafuInfo {
    Enum(EnumInfo),
    NamedStruct(NamedStructInfo),
    TupleStruct(TupleStructInfo),
}

struct EnumInfo {
    crate_root: UserInput,
    name: syn::Ident,
    generics: syn::Generics,
    variants: Vec<FieldContainer>,
    default_visibility: UserInput,
    module: Option<ModuleName>,
}

struct FieldContainer {
    name: syn::Ident,
    backtrace_field: Option<Field>,
    selector_kind: ContextSelectorKind,
    display_format: Option<UserInput>,
    doc_comment: String,
    visibility: Option<UserInput>,
    module: Option<ModuleName>,
}

enum SuffixKind {
    Default,
    None,
    Some(syn::Ident),
}

enum ContextSelectorKind {
    Context {
        suffix: SuffixKind,
        source_field: Option<SourceField>,
        user_fields: Vec<Field>,
    },

    Whatever {
        source_field: Option<SourceField>,
        message_field: Field,
    },

    NoContext {
        source_field: SourceField,
    },
}

impl ContextSelectorKind {
    fn is_whatever(&self) -> bool {
        match self {
            ContextSelectorKind::Whatever { .. } => true,
            _ => false,
        }
    }

    fn user_fields(&self) -> &[Field] {
        match self {
            ContextSelectorKind::Context { user_fields, .. } => user_fields,
            ContextSelectorKind::Whatever { .. } => &[],
            ContextSelectorKind::NoContext { .. } => &[],
        }
    }

    fn source_field(&self) -> Option<&SourceField> {
        match self {
            ContextSelectorKind::Context { source_field, .. } => source_field.as_ref(),
            ContextSelectorKind::Whatever { source_field, .. } => source_field.as_ref(),
            ContextSelectorKind::NoContext { source_field } => Some(source_field),
        }
    }

    fn message_field(&self) -> Option<&Field> {
        match self {
            ContextSelectorKind::Context { .. } => None,
            ContextSelectorKind::Whatever { message_field, .. } => Some(message_field),
            ContextSelectorKind::NoContext { .. } => None,
        }
    }
}

struct NamedStructInfo {
    crate_root: UserInput,
    field_container: FieldContainer,
    generics: syn::Generics,
}

struct TupleStructInfo {
    crate_root: UserInput,
    name: syn::Ident,
    generics: syn::Generics,
    transformation: Transformation,
}

#[derive(Clone)]
pub(crate) struct Field {
    name: syn::Ident,
    ty: syn::Type,
    original: syn::Field,
}

impl Field {
    fn name(&self) -> &syn::Ident {
        &self.name
    }
}

struct SourceField {
    name: syn::Ident,
    transformation: Transformation,
    backtrace_delegate: bool,
}

impl SourceField {
    fn name(&self) -> &syn::Ident {
        &self.name
    }
}

enum Transformation {
    None { ty: syn::Type },
    Transform { ty: syn::Type, expr: syn::Expr },
}

impl Transformation {
    fn ty(&self) -> &syn::Type {
        match self {
            Transformation::None { ty } => ty,
            Transformation::Transform { ty, .. } => ty,
        }
    }

    fn transformation(&self) -> proc_macro2::TokenStream {
        match self {
            Transformation::None { .. } => quote! { |v| v },
            Transformation::Transform { expr, .. } => quote! { #expr },
        }
    }
}

/// SyntaxErrors is a convenience wrapper for a list of syntax errors discovered while parsing
/// something that derives Snafu.  It makes it easier for developers to add and return syntax
/// errors while walking through the parse tree.
#[derive(Debug, Default)]
struct SyntaxErrors {
    inner: Vec<syn::Error>,
}

impl SyntaxErrors {
    /// Start a set of errors that all share the same location
    fn scoped(&mut self, scope: ErrorLocation) -> SyntaxErrorsScoped<'_> {
        SyntaxErrorsScoped {
            errors: self,
            scope,
        }
    }

    /// Adds a new syntax error. The description will be used in the
    /// compile error pointing to the tokens.
    fn add(&mut self, tokens: impl quote::ToTokens, description: impl fmt::Display) {
        self.inner
            .push(syn::Error::new_spanned(tokens, description));
    }

    /// Adds the given list of errors.
    fn extend(&mut self, errors: impl IntoIterator<Item = syn::Error>) {
        self.inner.extend(errors);
    }

    #[allow(dead_code)]
    /// Returns the number of errors that have been added.
    fn len(&self) -> usize {
        self.inner.len()
    }

    /// Consume the SyntaxErrors, returning Ok if there were no syntax errors added, or Err(list)
    /// if there were syntax errors.
    fn finish(self) -> MultiSynResult<()> {
        if self.inner.is_empty() {
            Ok(())
        } else {
            Err(self.inner)
        }
    }

    /// Consume the SyntaxErrors and a Result, returning the success
    /// value if neither have errors, otherwise combining the errors.
    fn absorb<T>(mut self, res: MultiSynResult<T>) -> MultiSynResult<T> {
        match res {
            Ok(v) => self.finish().map(|()| v),
            Err(e) => {
                self.inner.extend(e);
                Err(self.inner)
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum ErrorLocation {
    OnEnum,
    OnVariant,
    InVariant,
    OnField,
    OnNamedStruct,
    InNamedStruct,
    OnTupleStruct,
}

impl fmt::Display for ErrorLocation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use crate::ErrorLocation::*;

        match self {
            OnEnum => "on an enum".fmt(f),
            OnVariant => "on an enum variant".fmt(f),
            InVariant => "within an enum variant".fmt(f),
            OnField => "on a field".fmt(f),
            OnNamedStruct => "on a named struct".fmt(f),
            InNamedStruct => "within a named struct".fmt(f),
            OnTupleStruct => "on a tuple struct".fmt(f),
        }
    }
}

trait ErrorForLocation {
    fn for_location(&self, location: ErrorLocation) -> String;
}

struct SyntaxErrorsScoped<'a> {
    errors: &'a mut SyntaxErrors,
    scope: ErrorLocation,
}

impl SyntaxErrorsScoped<'_> {
    /// Adds a new syntax error. The description will be used in the
    /// compile error pointing to the tokens.
    fn add(&mut self, tokens: impl quote::ToTokens, description: impl ErrorForLocation) {
        let description = description.for_location(self.scope);
        self.errors.add(tokens, description)
    }
}

/// Helper structure to handle cases where an attribute was used on an
/// element where it's not valid.
#[derive(Debug)]
struct OnlyValidOn {
    /// The name of the attribute that was misused.
    attribute: &'static str,
    /// A description of where that attribute is valid.
    valid_on: &'static str,
}

impl ErrorForLocation for OnlyValidOn {
    fn for_location(&self, location: ErrorLocation) -> String {
        format!(
            "`{}` attribute is only valid on {}, not {}",
            self.attribute, self.valid_on, location,
        )
    }
}

/// Helper structure to handle cases where a specific attribute value
/// was used on an field where it's not valid.
#[derive(Debug)]
struct WrongField {
    /// The name of the attribute that was misused.
    attribute: &'static str,
    /// The name of the field where that attribute is valid.
    valid_field: &'static str,
}

impl ErrorForLocation for WrongField {
    fn for_location(&self, _location: ErrorLocation) -> String {
        format!(
            r#"`{}` attribute is only valid on a field named "{}", not on other fields"#,
            self.attribute, self.valid_field,
        )
    }
}

/// Helper structure to handle cases where two incompatible attributes
/// were specified on the same element.
#[derive(Debug)]
struct IncompatibleAttributes(&'static [&'static str]);

impl ErrorForLocation for IncompatibleAttributes {
    fn for_location(&self, location: ErrorLocation) -> String {
        let attrs_string = self
            .0
            .iter()
            .map(|attr| format!("`{}`", attr))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Incompatible attributes [{}] specified {}",
            attrs_string, location,
        )
    }
}

/// Helper structure to handle cases where an attribute was
/// incorrectly used multiple times on the same element.
#[derive(Debug)]
struct DuplicateAttribute {
    attribute: &'static str,
    location: ErrorLocation,
}

impl fmt::Display for DuplicateAttribute {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Multiple `{}` attributes are not supported {}",
            self.attribute, self.location,
        )
    }
}

/// AtMostOne is a helper to track attributes seen during parsing.  If more than one item is added,
/// it's added to a list of DuplicateAttribute errors, using the given `name` and `location` as
/// descriptors.
///
/// When done parsing a structure, call `finish` to get first attribute found, if any, and the list
/// of errors, or call `finish_with_location` to get the attribute and the token tree where it was
/// found, which can be useful for error reporting.
#[derive(Debug)]
struct AtMostOne<T, U>
where
    U: quote::ToTokens,
{
    name: &'static str,
    location: ErrorLocation,
    // We store all the values we've seen to allow for `iter`, which helps the `AtMostOne` be
    // useful for additional manual error checking.
    values: VecDeque<(T, U)>,
    errors: SyntaxErrors,
}

impl<T, U> AtMostOne<T, U>
where
    U: quote::ToTokens + Clone,
{
    /// Creates an AtMostOne to track an attribute with the given
    /// `name` on the given `location` (often referencing a parent
    /// element).
    fn new(name: &'static str, location: ErrorLocation) -> Self {
        Self {
            name,
            location,
            values: VecDeque::new(),
            errors: SyntaxErrors::default(),
        }
    }

    /// Add an occurence of the attribute found at the given token tree `tokens`.
    fn add(&mut self, item: T, tokens: U) {
        if !self.values.is_empty() {
            self.errors.add(
                tokens.clone(),
                DuplicateAttribute {
                    attribute: self.name,
                    location: self.location,
                },
            );
        }
        self.values.push_back((item, tokens));
    }

    #[allow(dead_code)]
    /// Returns the number of elements that have been added.
    fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns true if no elements have been added, otherwise false.
    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns an iterator over all values that have been added.
    ///
    /// This can help with additional manual error checks beyond the duplication checks that
    /// `AtMostOne` handles for you.
    fn iter(&self) -> std::collections::vec_deque::Iter<(T, U)> {
        self.values.iter()
    }

    /// Consumes the AtMostOne, returning the first item added, if any, and the list of errors
    /// representing any items added beyond the first.
    fn finish(self) -> (Option<T>, Vec<syn::Error>) {
        let (value, errors) = self.finish_with_location();
        (value.map(|(val, _location)| val), errors)
    }

    /// Like `finish` but also returns the location of the first item added.  Useful when you have
    /// to do additional, manual error checking on the first item added, and you'd like to report
    /// an accurate location for it in case of errors.
    fn finish_with_location(mut self) -> (Option<(T, U)>, Vec<syn::Error>) {
        let errors = match self.errors.finish() {
            Ok(()) => Vec::new(),
            Err(vec) => vec,
        };
        (self.values.pop_front(), errors)
    }
}

fn impl_snafu_macro(ty: syn::DeriveInput) -> TokenStream {
    match parse_snafu_information(ty) {
        Ok(info) => info.into(),
        Err(e) => to_compile_errors(e).into(),
    }
}

fn to_compile_errors(errors: Vec<syn::Error>) -> proc_macro2::TokenStream {
    let compile_errors = errors.iter().map(syn::Error::to_compile_error);
    quote! { #(#compile_errors)* }
}

fn parse_snafu_information(ty: syn::DeriveInput) -> MultiSynResult<SnafuInfo> {
    use syn::spanned::Spanned;
    use syn::Data;

    let span = ty.span();
    let syn::DeriveInput {
        ident,
        generics,
        data,
        attrs,
        ..
    } = ty;

    match data {
        Data::Enum(enum_) => parse_snafu_enum(enum_, ident, generics, attrs).map(SnafuInfo::Enum),
        Data::Struct(struct_) => parse_snafu_struct(struct_, ident, generics, attrs, span),
        _ => Err(vec![syn::Error::new(
            span,
            "Can only derive `Snafu` for an enum or a newtype",
        )]),
    }
}

const ATTR_DISPLAY: OnlyValidOn = OnlyValidOn {
    attribute: "display",
    valid_on: "enum variants or structs with named fields",
};

const ATTR_SOURCE: OnlyValidOn = OnlyValidOn {
    attribute: "source",
    valid_on: "enum variant or struct fields with a name",
};

const ATTR_SOURCE_BOOL: OnlyValidOn = OnlyValidOn {
    attribute: "source(bool)",
    valid_on: "enum variant or struct fields with a name",
};

const ATTR_SOURCE_FALSE: WrongField = WrongField {
    attribute: "source(false)",
    valid_field: "source",
};

const ATTR_SOURCE_FROM: OnlyValidOn = OnlyValidOn {
    attribute: "source(from)",
    valid_on: "enum variant or struct fields with a name",
};

const ATTR_BACKTRACE: OnlyValidOn = OnlyValidOn {
    attribute: "backtrace",
    valid_on: "enum variant or struct fields with a name",
};

const ATTR_BACKTRACE_FALSE: WrongField = WrongField {
    attribute: "backtrace(false)",
    valid_field: "backtrace",
};

const ATTR_VISIBILITY: OnlyValidOn = OnlyValidOn {
    attribute: "visibility",
    valid_on: "an enum, enum variants, or a struct with named fields",
};

const ATTR_MODULE: OnlyValidOn = OnlyValidOn {
    attribute: "module",
    valid_on: "an enum or structs with named fields",
};

const ATTR_CONTEXT: OnlyValidOn = OnlyValidOn {
    attribute: "context",
    valid_on: "enum variants or structs with named fields",
};

const ATTR_WHATEVER: OnlyValidOn = OnlyValidOn {
    attribute: "whatever",
    valid_on: "enum variants or structs with named fields",
};

const ATTR_CRATE_ROOT: OnlyValidOn = OnlyValidOn {
    attribute: "crate_root",
    valid_on: "an enum or a struct",
};

const SOURCE_BOOL_FROM_INCOMPATIBLE: IncompatibleAttributes =
    IncompatibleAttributes(&["source(false)", "source(from)"]);

fn parse_snafu_enum(
    enum_: syn::DataEnum,
    name: syn::Ident,
    generics: syn::Generics,
    attrs: Vec<syn::Attribute>,
) -> MultiSynResult<EnumInfo> {
    use syn::spanned::Spanned;
    use syn::Fields;

    let mut errors = SyntaxErrors::default();

    let mut modules = AtMostOne::new("module", ErrorLocation::OnEnum);
    let mut default_visibilities = AtMostOne::new("visibility", ErrorLocation::OnEnum);
    let mut crate_roots = AtMostOne::new("crate_root", ErrorLocation::OnEnum);
    let mut enum_errors = errors.scoped(ErrorLocation::OnEnum);

    for attr in attributes_from_syn(attrs)? {
        match attr {
            SnafuAttribute::Visibility(tokens, v) => {
                default_visibilities.add(v, tokens);
            }
            SnafuAttribute::Display(tokens, ..) => enum_errors.add(tokens, ATTR_DISPLAY),
            SnafuAttribute::Source(tokens, ss) => {
                for s in ss {
                    match s {
                        Source::Flag(..) => enum_errors.add(tokens.clone(), ATTR_SOURCE_BOOL),
                        Source::From(..) => enum_errors.add(tokens.clone(), ATTR_SOURCE_FROM),
                    }
                }
            }
            SnafuAttribute::CrateRoot(tokens, root) => {
                crate_roots.add(root, tokens);
            }
            SnafuAttribute::Module(tokens, v) => modules.add(v, tokens),
            SnafuAttribute::Backtrace(tokens, ..) => enum_errors.add(tokens, ATTR_BACKTRACE),
            SnafuAttribute::Context(tokens, ..) => enum_errors.add(tokens, ATTR_CONTEXT),
            SnafuAttribute::Whatever(tokens) => enum_errors.add(tokens, ATTR_WHATEVER),
            SnafuAttribute::DocComment(..) => { /* Just a regular doc comment. */ }
        }
    }

    let (module, errs) = modules.finish();
    errors.extend(errs);

    let default_default_visibility = match module {
        Some(_) => {
            // Default to pub visibility since private context selectors wouldn't
            // be accessible outside the module.
            pub_visibility
        }
        None => private_visibility,
    };

    let (maybe_default_visibility, errs) = default_visibilities.finish();
    let default_visibility = maybe_default_visibility.unwrap_or_else(default_default_visibility);
    errors.extend(errs);

    let (maybe_crate_root, errs) = crate_roots.finish();
    let crate_root = maybe_crate_root.unwrap_or_else(default_crate_root);
    errors.extend(errs);

    let variants: sponge::AllErrors<_, _> = enum_
        .variants
        .into_iter()
        .map(|variant| {
            let fields = match variant.fields {
                Fields::Named(f) => f.named.into_iter().collect(),
                Fields::Unnamed(_) => {
                    return Err(vec![syn::Error::new(
                        variant.fields.span(),
                        "Only struct-like and unit enum variants are supported",
                    )]);
                }
                Fields::Unit => vec![],
            };

            let name = variant.ident;
            let span = name.span();

            let attrs = attributes_from_syn(variant.attrs)?;

            field_container(
                name,
                span,
                attrs,
                fields,
                &mut errors,
                ErrorLocation::OnVariant,
                ErrorLocation::InVariant,
            )
        })
        .collect();

    let variants = errors.absorb(variants.into_result())?;

    Ok(EnumInfo {
        crate_root,
        name,
        generics,
        variants,
        default_visibility,
        module,
    })
}

fn field_container(
    name: syn::Ident,
    variant_span: proc_macro2::Span,
    attrs: Vec<SnafuAttribute>,
    fields: Vec<syn::Field>,
    errors: &mut SyntaxErrors,
    outer_error_location: ErrorLocation,
    inner_error_location: ErrorLocation,
) -> MultiSynResult<FieldContainer> {
    use quote::ToTokens;
    use syn::spanned::Spanned;

    let mut outer_errors = errors.scoped(outer_error_location);

    let mut modules = AtMostOne::new("module", outer_error_location);
    let mut display_formats = AtMostOne::new("display", outer_error_location);
    let mut visibilities = AtMostOne::new("visibility", outer_error_location);
    let mut contexts = AtMostOne::new("context", outer_error_location);
    let mut whatevers = AtMostOne::new("whatever", outer_error_location);
    let mut doc_comment = String::new();
    let mut reached_end_of_doc_comment = false;

    for attr in attrs {
        match attr {
            SnafuAttribute::Module(tokens, n) => modules.add(n, tokens),
            SnafuAttribute::Display(tokens, d) => display_formats.add(d, tokens),
            SnafuAttribute::Visibility(tokens, v) => visibilities.add(v, tokens),
            SnafuAttribute::Context(tokens, c) => contexts.add(c, tokens),
            SnafuAttribute::Whatever(tokens) => whatevers.add((), tokens),
            SnafuAttribute::Source(tokens, ..) => outer_errors.add(tokens, ATTR_SOURCE),
            SnafuAttribute::Backtrace(tokens, ..) => outer_errors.add(tokens, ATTR_BACKTRACE),
            SnafuAttribute::CrateRoot(tokens, ..) => outer_errors.add(tokens, ATTR_CRATE_ROOT),
            SnafuAttribute::DocComment(_tts, doc_comment_line) => {
                // We join all the doc comment attributes with a space,
                // but end once the summary of the doc comment is
                // complete, which is indicated by an empty line.
                if !reached_end_of_doc_comment {
                    let trimmed = doc_comment_line.trim();
                    if trimmed.is_empty() {
                        reached_end_of_doc_comment = true;
                    } else {
                        if !doc_comment.is_empty() {
                            doc_comment.push_str(" ");
                        }
                        doc_comment.push_str(trimmed);
                    }
                }
            }
        }
    }

    let mut user_fields = Vec::new();
    let mut source_fields = AtMostOne::new("source", inner_error_location);
    let mut backtrace_fields = AtMostOne::new("backtrace", inner_error_location);

    for syn_field in fields {
        let original = syn_field.clone();
        let span = syn_field.span();
        let name = syn_field
            .ident
            .as_ref()
            .ok_or_else(|| vec![syn::Error::new(span, "Must have a named field")])?;
        let field = Field {
            name: name.clone(),
            ty: syn_field.ty.clone(),
            original,
        };

        // Check whether we have multiple source/backtrace attributes on this field.
        // We can't just add to source_fields/backtrace_fields from inside the attribute
        // loop because source and backtrace are connected and require a bit of special
        // logic after the attribute loop.  For example, we need to know whether there's a
        // source transformation before we record a source field, but it might be on a
        // later attribute.  We use the data field of `source_attrs` to track any
        // transformations in case it was a `source(from(...))`, but for backtraces we
        // don't need any more data.
        let mut source_attrs = AtMostOne::new("source", ErrorLocation::OnField);
        let mut backtrace_attrs = AtMostOne::new("backtrace", ErrorLocation::OnField);

        // Keep track of the negative markers so we can check for inconsistencies and
        // exclude fields even if they have the "source" or "backtrace" name.
        let mut source_opt_out = false;
        let mut backtrace_opt_out = false;

        let mut field_errors = errors.scoped(ErrorLocation::OnField);

        for attr in attributes_from_syn(syn_field.attrs.clone())? {
            match attr {
                SnafuAttribute::Source(tokens, ss) => {
                    for s in ss {
                        match s {
                            Source::Flag(v) => {
                                // If we've seen a `source(from)` then there will be a
                                // `Some` value in `source_attrs`.
                                let seen_source_from = source_attrs
                                    .iter()
                                    .map(|(val, _location)| val)
                                    .any(Option::is_some);
                                if !v && seen_source_from {
                                    field_errors.add(tokens.clone(), SOURCE_BOOL_FROM_INCOMPATIBLE);
                                }
                                if v {
                                    source_attrs.add(None, tokens.clone());
                                } else if name == "source" {
                                    source_opt_out = true;
                                } else {
                                    field_errors.add(tokens.clone(), ATTR_SOURCE_FALSE);
                                }
                            }
                            Source::From(t, e) => {
                                if source_opt_out {
                                    field_errors.add(tokens.clone(), SOURCE_BOOL_FROM_INCOMPATIBLE);
                                }
                                source_attrs.add(Some((t, e)), tokens.clone());
                            }
                        }
                    }
                }
                SnafuAttribute::Backtrace(tokens, v) => {
                    if v {
                        backtrace_attrs.add((), tokens);
                    } else if name == "backtrace" {
                        backtrace_opt_out = true;
                    } else {
                        field_errors.add(tokens, ATTR_BACKTRACE_FALSE);
                    }
                }
                SnafuAttribute::Module(tokens, ..) => field_errors.add(tokens, ATTR_MODULE),
                SnafuAttribute::Visibility(tokens, ..) => field_errors.add(tokens, ATTR_VISIBILITY),
                SnafuAttribute::Display(tokens, ..) => field_errors.add(tokens, ATTR_DISPLAY),
                SnafuAttribute::Context(tokens, ..) => field_errors.add(tokens, ATTR_CONTEXT),
                SnafuAttribute::Whatever(tokens) => field_errors.add(tokens, ATTR_WHATEVER),
                SnafuAttribute::CrateRoot(tokens, ..) => field_errors.add(tokens, ATTR_CRATE_ROOT),
                SnafuAttribute::DocComment(..) => { /* Just a regular doc comment. */ }
            }
        }

        // Add errors for any duplicated attributes on this field.
        let (source_attr, errs) = source_attrs.finish_with_location();
        errors.extend(errs);
        let (backtrace_attr, errs) = backtrace_attrs.finish_with_location();
        errors.extend(errs);

        let source_attr = source_attr.or_else(|| {
            if field.name == "source" && !source_opt_out {
                Some((None, syn_field.clone().into_token_stream()))
            } else {
                None
            }
        });

        let backtrace_attr = backtrace_attr.or_else(|| {
            if field.name == "backtrace" && !backtrace_opt_out {
                Some(((), syn_field.clone().into_token_stream()))
            } else {
                None
            }
        });

        if let Some((maybe_transformation, location)) = source_attr {
            let Field { name, ty, .. } = field;
            let transformation = maybe_transformation
                .map(|(ty, expr)| Transformation::Transform { ty, expr })
                .unwrap_or_else(|| Transformation::None { ty });

            source_fields.add(
                SourceField {
                    name,
                    transformation,
                    // Specifying `backtrace` on a source field is how you request
                    // delegation of the backtrace to the source error type.
                    backtrace_delegate: backtrace_attr.is_some(),
                },
                location,
            );
        } else if let Some((_, location)) = backtrace_attr {
            backtrace_fields.add(field, location);
        } else {
            user_fields.push(field);
        }
    }

    let (source, errs) = source_fields.finish_with_location();
    errors.extend(errs);

    let (backtrace, errs) = backtrace_fields.finish_with_location();
    errors.extend(errs);

    match (&source, &backtrace) {
        (Some(source), Some(backtrace)) if source.0.backtrace_delegate => {
            let source_location = source.1.clone();
            let backtrace_location = backtrace.1.clone();
            errors.add(
                source_location,
                "Cannot have `backtrace` field and `backtrace` attribute on a source field in the same variant",
            );
            errors.add(
                backtrace_location,
                "Cannot have `backtrace` field and `backtrace` attribute on a source field in the same variant",
            );
        }
        _ => {} // no conflict
    }

    let (module, errs) = modules.finish();
    errors.extend(errs);

    let (display_format, errs) = display_formats.finish();
    errors.extend(errs);

    let (visibility, errs) = visibilities.finish();
    errors.extend(errs);

    let (is_context, errs) = contexts.finish_with_location();
    let is_context = is_context.map(|(c, tt)| (c.into_enabled(), tt));
    errors.extend(errs);

    let (is_whatever, errs) = whatevers.finish_with_location();
    errors.extend(errs);

    let source_field = source.map(|(val, _tts)| val);

    let selector_kind = match (is_context, is_whatever) {
        (Some(((true, _), c_tt)), Some(((), o_tt))) => {
            let txt = "Cannot be both a `context` and `whatever` error";
            return Err(vec![
                syn::Error::new_spanned(c_tt, txt),
                syn::Error::new_spanned(o_tt, txt),
            ]);
        }

        (Some(((true, suffix), _)), None) => ContextSelectorKind::Context {
            suffix,
            source_field,
            user_fields,
        },

        (None, None) => ContextSelectorKind::Context {
            suffix: SuffixKind::Default,
            source_field,
            user_fields,
        },

        (Some(((false, _), _)), Some(_)) | (None, Some(_)) => {
            let mut messages = AtMostOne::new("message", outer_error_location);

            for f in user_fields {
                if f.name == "message" {
                    let l = f.original.clone();
                    messages.add(f, l);
                } else {
                    errors.add(
                        f.original,
                        "Whatever selectors must not have context fields",
                    );
                    // todo: phrasing?
                }
            }

            let (message_field, errs) = messages.finish();
            errors.extend(errs);

            let message_field = message_field.ok_or_else(|| {
                vec![syn::Error::new(
                    variant_span,
                    "Whatever selectors must have a message field",
                )]
            })?;

            ContextSelectorKind::Whatever {
                source_field,
                message_field,
            }
        }

        (Some(((false, _), _)), None) => {
            errors.extend(user_fields.into_iter().map(|Field { original, .. }| {
                syn::Error::new_spanned(
                    original,
                    "Context selectors without context must not have context fields",
                )
            }));

            let source_field = source_field.ok_or_else(|| {
                vec![syn::Error::new(
                    variant_span,
                    "Context selectors without context must have a source field",
                )]
            })?;

            ContextSelectorKind::NoContext { source_field }
        }
    };

    Ok(FieldContainer {
        name,
        backtrace_field: backtrace.map(|(val, _tts)| val),
        selector_kind,
        display_format,
        doc_comment,
        visibility,
        module,
    })
}

fn parse_snafu_struct(
    struct_: syn::DataStruct,
    name: syn::Ident,
    generics: syn::Generics,
    attrs: Vec<syn::Attribute>,
    span: proc_macro2::Span,
) -> MultiSynResult<SnafuInfo> {
    use syn::Fields;

    match struct_.fields {
        Fields::Named(f) => {
            let f = f.named.into_iter().collect();
            parse_snafu_named_struct(f, name, generics, attrs, span).map(SnafuInfo::NamedStruct)
        }
        Fields::Unnamed(f) => {
            parse_snafu_tuple_struct(f, name, generics, attrs, span).map(SnafuInfo::TupleStruct)
        }
        Fields::Unit => parse_snafu_named_struct(vec![], name, generics, attrs, span)
            .map(SnafuInfo::NamedStruct),
    }
}

fn parse_snafu_named_struct(
    fields: Vec<syn::Field>,
    name: syn::Ident,
    generics: syn::Generics,
    attrs: Vec<syn::Attribute>,
    span: proc_macro2::Span,
) -> MultiSynResult<NamedStructInfo> {
    let mut errors = SyntaxErrors::default();

    let attrs = attributes_from_syn(attrs)?;

    let mut crate_roots = AtMostOne::new("crate_root", ErrorLocation::OnNamedStruct);

    let attrs = attrs
        .into_iter()
        .flat_map(|attr| match attr {
            SnafuAttribute::CrateRoot(tokens, root) => {
                crate_roots.add(root, tokens);
                None
            }
            other => Some(other),
        })
        .collect();

    let field_container = field_container(
        name,
        span,
        attrs,
        fields,
        &mut errors,
        ErrorLocation::OnNamedStruct,
        ErrorLocation::InNamedStruct,
    )?;

    let (maybe_crate_root, errs) = crate_roots.finish();
    let crate_root = maybe_crate_root.unwrap_or_else(default_crate_root);
    errors.extend(errs);

    errors.finish()?;

    Ok(NamedStructInfo {
        crate_root,
        field_container,
        generics,
    })
}

fn parse_snafu_tuple_struct(
    mut fields: syn::FieldsUnnamed,
    name: syn::Ident,
    generics: syn::Generics,
    attrs: Vec<syn::Attribute>,
    span: proc_macro2::Span,
) -> MultiSynResult<TupleStructInfo> {
    let mut transformations = AtMostOne::new("source(from)", ErrorLocation::OnTupleStruct);
    let mut crate_roots = AtMostOne::new("crate_root", ErrorLocation::OnTupleStruct);

    let mut errors = SyntaxErrors::default();
    let mut struct_errors = errors.scoped(ErrorLocation::OnTupleStruct);

    for attr in attributes_from_syn(attrs)? {
        match attr {
            SnafuAttribute::Module(tokens, ..) => struct_errors.add(tokens, ATTR_MODULE),
            SnafuAttribute::Display(tokens, ..) => struct_errors.add(tokens, ATTR_DISPLAY),
            SnafuAttribute::Visibility(tokens, ..) => struct_errors.add(tokens, ATTR_VISIBILITY),
            SnafuAttribute::Source(tokens, ss) => {
                for s in ss {
                    match s {
                        Source::Flag(..) => struct_errors.add(tokens.clone(), ATTR_SOURCE_BOOL),
                        Source::From(t, e) => transformations.add((t, e), tokens.clone()),
                    }
                }
            }
            SnafuAttribute::Backtrace(tokens, ..) => struct_errors.add(tokens, ATTR_BACKTRACE),
            SnafuAttribute::Context(tokens, ..) => struct_errors.add(tokens, ATTR_CONTEXT),
            SnafuAttribute::Whatever(tokens) => struct_errors.add(tokens, ATTR_CONTEXT),
            SnafuAttribute::CrateRoot(tokens, root) => crate_roots.add(root, tokens),
            SnafuAttribute::DocComment(..) => { /* Just a regular doc comment. */ }
        }
    }

    fn one_field_error(span: proc_macro2::Span) -> syn::Error {
        syn::Error::new(
            span,
            "Can only derive `Snafu` for tuple structs with exactly one field",
        )
    }

    let inner = fields
        .unnamed
        .pop()
        .ok_or_else(|| vec![one_field_error(span)])?;
    if !fields.unnamed.is_empty() {
        return Err(vec![one_field_error(span)]);
    }

    let (maybe_transformation, errs) = transformations.finish();
    let transformation = maybe_transformation
        .map(|(ty, expr)| Transformation::Transform { ty, expr })
        .unwrap_or_else(|| Transformation::None {
            ty: inner.into_value().ty,
        });
    errors.extend(errs);

    let (maybe_crate_root, errs) = crate_roots.finish();
    let crate_root = maybe_crate_root.unwrap_or_else(default_crate_root);
    errors.extend(errs);

    errors.finish()?;

    Ok(TupleStructInfo {
        crate_root,
        name,
        generics,
        transformation,
    })
}

enum Context {
    Flag(bool),
    Suffix(SuffixKind),
}

impl Context {
    fn into_enabled(self) -> (bool, SuffixKind) {
        match self {
            Context::Flag(b) => (b, SuffixKind::None),
            Context::Suffix(suffix) => (true, suffix),
        }
    }
}

enum Source {
    Flag(bool),
    From(syn::Type, syn::Expr),
}

/// A SnafuAttribute represents one SNAFU-specific attribute inside of `#[snafu(...)]`.  For
/// example, in `#[snafu(visibility(pub), display("hi"))]`, `visibility(pub)` and `display("hi")`
/// are each a SnafuAttribute.
///
/// We store the location in the source where we found the attribute (as a `TokenStream`) along
/// with the data.  The location can be used to give accurate error messages in case there was a
/// problem with the use of the attribute.
enum SnafuAttribute {
    Display(proc_macro2::TokenStream, UserInput),
    Visibility(proc_macro2::TokenStream, UserInput),
    Source(proc_macro2::TokenStream, Vec<Source>),
    Backtrace(proc_macro2::TokenStream, bool),
    Context(proc_macro2::TokenStream, Context),
    Whatever(proc_macro2::TokenStream),
    CrateRoot(proc_macro2::TokenStream, UserInput),
    DocComment(proc_macro2::TokenStream, String),
    Module(proc_macro2::TokenStream, ModuleName),
}

fn default_crate_root() -> UserInput {
    Box::new(quote! { ::snafu })
}

fn private_visibility() -> UserInput {
    Box::new(quote! {})
}

fn pub_visibility() -> UserInput {
    Box::new(syn::token::Pub(proc_macro2::Span::call_site()))
}

impl From<SnafuInfo> for proc_macro::TokenStream {
    fn from(other: SnafuInfo) -> proc_macro::TokenStream {
        match other {
            SnafuInfo::Enum(e) => e.into(),
            SnafuInfo::NamedStruct(s) => s.into(),
            SnafuInfo::TupleStruct(s) => s.into(),
        }
    }
}

impl From<EnumInfo> for proc_macro::TokenStream {
    fn from(other: EnumInfo) -> proc_macro::TokenStream {
        other.generate_snafu().into()
    }
}

impl From<NamedStructInfo> for proc_macro::TokenStream {
    fn from(other: NamedStructInfo) -> proc_macro::TokenStream {
        other.generate_snafu().into()
    }
}

impl From<TupleStructInfo> for proc_macro::TokenStream {
    fn from(other: TupleStructInfo) -> proc_macro::TokenStream {
        other.generate_snafu().into()
    }
}

trait GenericAwareNames {
    fn name(&self) -> &syn::Ident;

    fn generics(&self) -> &syn::Generics;

    fn parameterized_name(&self) -> UserInput {
        let enum_name = self.name();
        let original_generics = self.provided_generic_names();

        Box::new(quote! { #enum_name<#(#original_generics,)*> })
    }

    fn provided_generic_types_without_defaults(&self) -> Vec<proc_macro2::TokenStream> {
        use syn::TypeParam;
        self.generics()
            .type_params()
            .map(|t: &TypeParam| {
                let TypeParam {
                    attrs,
                    ident,
                    colon_token,
                    bounds,
                    ..
                } = t;
                quote! {
                    #(#attrs)*
                    #ident
                    #colon_token
                    #bounds
                }
            })
            .collect()
    }

    fn provided_generics_without_defaults(&self) -> Vec<proc_macro2::TokenStream> {
        self.provided_generic_lifetimes()
            .into_iter()
            .chain(self.provided_generic_types_without_defaults().into_iter())
            .collect()
    }

    fn provided_generic_lifetimes(&self) -> Vec<proc_macro2::TokenStream> {
        use syn::{GenericParam, LifetimeDef};

        self.generics()
            .params
            .iter()
            .flat_map(|p| match p {
                GenericParam::Lifetime(LifetimeDef { lifetime, .. }) => Some(quote! { #lifetime }),
                _ => None,
            })
            .collect()
    }

    fn provided_generic_names(&self) -> Vec<proc_macro2::TokenStream> {
        use syn::{ConstParam, GenericParam, LifetimeDef, TypeParam};

        self.generics()
            .params
            .iter()
            .map(|p| match p {
                GenericParam::Type(TypeParam { ident, .. }) => quote! { #ident },
                GenericParam::Lifetime(LifetimeDef { lifetime, .. }) => quote! { #lifetime },
                GenericParam::Const(ConstParam { ident, .. }) => quote! { #ident },
            })
            .collect()
    }

    fn provided_where_clauses(&self) -> Vec<proc_macro2::TokenStream> {
        self.generics()
            .where_clause
            .iter()
            .flat_map(|c| c.predicates.iter().map(|p| quote! { #p }))
            .collect()
    }
}

impl EnumInfo {
    fn generate_snafu(self) -> proc_macro2::TokenStream {
        let context_selectors = ContextSelectors(&self);
        let display_impl = DisplayImpl(&self);
        let error_impl = ErrorImpl(&self);
        let error_compat_impl = ErrorCompatImpl(&self);

        let context = match self.module {
            None => quote! { #context_selectors },
            Some(ref module_name) => {
                use crate::shared::ContextModule;

                let context_module = ContextModule {
                    container_name: self.name(),
                    body: &context_selectors,
                    visibility: Some(&self.default_visibility),
                    module_name,
                };

                quote! { #context_module }
            }
        };

        quote! {
            #context
            #display_impl
            #error_impl
            #error_compat_impl
        }
    }
}

impl GenericAwareNames for EnumInfo {
    fn name(&self) -> &syn::Ident {
        &self.name
    }

    fn generics(&self) -> &syn::Generics {
        &self.generics
    }
}

struct ContextSelectors<'a>(&'a EnumInfo);

impl<'a> quote::ToTokens for ContextSelectors<'a> {
    fn to_tokens(&self, stream: &mut proc_macro2::TokenStream) {
        let context_selectors = self
            .0
            .variants
            .iter()
            .map(|variant| ContextSelector(self.0, variant));

        stream.extend({
            quote! {
                #(#context_selectors)*
            }
        })
    }
}

struct ContextSelector<'a>(&'a EnumInfo, &'a FieldContainer);

impl<'a> quote::ToTokens for ContextSelector<'a> {
    fn to_tokens(&self, stream: &mut proc_macro2::TokenStream) {
        use crate::shared::ContextSelector;

        let enum_name = &self.0.name;

        let FieldContainer {
            name: variant_name,
            selector_kind,
            ..
        } = self.1;

        let visibility = self
            .1
            .visibility
            .as_ref()
            .unwrap_or(&self.0.default_visibility);

        let selector_doc_string = format!(
            "SNAFU context selector for the `{}::{}` variant",
            enum_name, variant_name,
        );

        let context_selector = ContextSelector {
            backtrace_field: self.1.backtrace_field.as_ref(),
            crate_root: &self.0.crate_root,
            error_constructor_name: &quote! { #enum_name::#variant_name },
            original_generics_without_defaults: &self.0.provided_generics_without_defaults(),
            parameterized_error_name: &self.0.parameterized_name(),
            selector_doc_string: &selector_doc_string,
            selector_kind: &selector_kind,
            selector_name: variant_name,
            user_fields: &selector_kind.user_fields(),
            visibility: Some(&visibility),
            where_clauses: &self.0.provided_where_clauses(),
        };

        stream.extend(quote! { #context_selector });
    }
}

struct DisplayImpl<'a>(&'a EnumInfo);

impl<'a> quote::ToTokens for DisplayImpl<'a> {
    fn to_tokens(&self, stream: &mut proc_macro2::TokenStream) {
        use self::shared::{Display, DisplayMatchArm};

        let enum_name = &self.0.name;

        let arms: Vec<_> = self
            .0
            .variants
            .iter()
            .map(|variant| {
                let FieldContainer {
                    backtrace_field,
                    display_format,
                    doc_comment,
                    name: variant_name,
                    selector_kind,
                    ..
                } = variant;

                let arm = DisplayMatchArm {
                    backtrace_field: backtrace_field.as_ref(),
                    default_name: &variant_name,
                    display_format: display_format.as_ref().map(|f| &**f),
                    doc_comment,
                    pattern_ident: &quote! { #enum_name::#variant_name },
                    selector_kind,
                };

                quote! { #arm }
            })
            .collect();

        let display = Display {
            arms: &arms,
            original_generics: &self.0.provided_generics_without_defaults(),
            parameterized_error_name: &self.0.parameterized_name(),
            where_clauses: &self.0.provided_where_clauses(),
        };

        let display_impl = quote! { #display };

        stream.extend(display_impl)
    }
}

struct ErrorImpl<'a>(&'a EnumInfo);

impl<'a> quote::ToTokens for ErrorImpl<'a> {
    fn to_tokens(&self, stream: &mut proc_macro2::TokenStream) {
        use self::shared::{Error, ErrorSourceMatchArm};

        let (variants_to_description, variants_to_source): (Vec<_>, Vec<_>) = self
            .0
            .variants
            .iter()
            .map(|field_container| {
                let enum_name = &self.0.name;
                let variant_name = &field_container.name;
                let pattern_ident = &quote! { #enum_name::#variant_name };

                let error_description_match_arm = quote! {
                    #pattern_ident { .. } => stringify!(#pattern_ident),
                };

                let error_source_match_arm = ErrorSourceMatchArm {
                    field_container,
                    pattern_ident,
                };
                let error_source_match_arm = quote! { #error_source_match_arm };

                (error_description_match_arm, error_source_match_arm)
            })
            .unzip();

        let error_impl = Error {
            crate_root: &self.0.crate_root,
            parameterized_error_name: &self.0.parameterized_name(),
            description_arms: &variants_to_description,
            source_arms: &variants_to_source,
            original_generics: &self.0.provided_generics_without_defaults(),
            where_clauses: &self.0.provided_where_clauses(),
        };
        let error_impl = quote! { #error_impl };

        stream.extend(error_impl);
    }
}

struct ErrorCompatImpl<'a>(&'a EnumInfo);

impl<'a> quote::ToTokens for ErrorCompatImpl<'a> {
    fn to_tokens(&self, stream: &mut proc_macro2::TokenStream) {
        use self::shared::{ErrorCompat, ErrorCompatBacktraceMatchArm};

        let variants_to_backtrace: Vec<_> = self
            .0
            .variants
            .iter()
            .map(|field_container| {
                let crate_root = &self.0.crate_root;
                let enum_name = &self.0.name;
                let variant_name = &field_container.name;

                let match_arm = ErrorCompatBacktraceMatchArm {
                    field_container,
                    crate_root,
                    pattern_ident: &quote! { #enum_name::#variant_name },
                };

                quote! { #match_arm }
            })
            .collect();

        let error_compat_impl = ErrorCompat {
            crate_root: &self.0.crate_root,
            parameterized_error_name: &self.0.parameterized_name(),
            backtrace_arms: &variants_to_backtrace,
            original_generics: &self.0.provided_generics_without_defaults(),
            where_clauses: &self.0.provided_where_clauses(),
        };

        let error_compat_impl = quote! { #error_compat_impl };

        stream.extend(error_compat_impl);
    }
}

impl NamedStructInfo {
    fn generate_snafu(self) -> proc_macro2::TokenStream {
        let parameterized_struct_name = self.parameterized_name();
        let original_generics = self.provided_generics_without_defaults();
        let where_clauses = self.provided_where_clauses();

        let Self {
            crate_root,
            field_container:
                FieldContainer {
                    name,
                    selector_kind,
                    backtrace_field,
                    display_format,
                    doc_comment,
                    visibility,
                    module,
                },
            ..
        } = &self;
        let field_container = &self.field_container;

        let user_fields = selector_kind.user_fields();

        use crate::shared::{Error, ErrorSourceMatchArm};

        let pattern_ident = &quote! { Self };

        let error_description_match_arm = quote! {
            #pattern_ident { .. } => stringify!(#name),
        };

        let error_source_match_arm = ErrorSourceMatchArm {
            field_container: &field_container,
            pattern_ident,
        };
        let error_source_match_arm = quote! { #error_source_match_arm };

        let error_impl = Error {
            crate_root: &crate_root,
            parameterized_error_name: &parameterized_struct_name,
            description_arms: &[error_description_match_arm],
            source_arms: &[error_source_match_arm],
            original_generics: &original_generics,
            where_clauses: &where_clauses,
        };
        let error_impl = quote! { #error_impl };

        use self::shared::{ErrorCompat, ErrorCompatBacktraceMatchArm};

        let match_arm = ErrorCompatBacktraceMatchArm {
            field_container,
            crate_root: &crate_root,
            pattern_ident: &quote! { Self },
        };
        let match_arm = quote! { #match_arm };

        let error_compat_impl = ErrorCompat {
            crate_root: &crate_root,
            parameterized_error_name: &parameterized_struct_name,
            backtrace_arms: &[match_arm],
            original_generics: &original_generics,
            where_clauses: &where_clauses,
        };

        use crate::shared::{Display, DisplayMatchArm};

        let arm = DisplayMatchArm {
            backtrace_field: backtrace_field.as_ref(),
            default_name: &name,
            display_format: display_format.as_ref().map(|f| &**f),
            doc_comment: &doc_comment,
            pattern_ident: &quote! { Self },
            selector_kind: &selector_kind,
        };
        let arm = quote! { #arm };

        let display_impl = Display {
            arms: &[arm],
            original_generics: &original_generics,
            parameterized_error_name: &parameterized_struct_name,
            where_clauses: &where_clauses,
        };

        use crate::shared::ContextSelector;

        let selector_doc_string = format!("SNAFU context selector for the `{}` error", name);

        let pub_visibility = pub_visibility();
        let selector_visibility = match (visibility, module) {
            (Some(ref v), _) => Some(&**v),
            (None, Some(_)) => {
                // Default to pub visibility since private context selectors
                // wouldn't be accessible outside the module.
                Some(&pub_visibility as &dyn quote::ToTokens)
            }
            (None, None) => None,
        };

        let context_selector = ContextSelector {
            backtrace_field: backtrace_field.as_ref(),
            crate_root: &crate_root,
            error_constructor_name: &name,
            original_generics_without_defaults: &original_generics,
            parameterized_error_name: &parameterized_struct_name,
            selector_doc_string: &selector_doc_string,
            selector_kind: &selector_kind,
            selector_name: &field_container.name,
            user_fields: &user_fields,
            visibility: selector_visibility,
            where_clauses: &where_clauses,
        };

        let context = match module {
            None => quote! { #context_selector },
            Some(module_name) => {
                use crate::shared::ContextModule;

                let context_module = ContextModule {
                    container_name: self.name(),
                    body: &context_selector,
                    visibility: visibility.as_ref().map(|x| &**x),
                    module_name,
                };

                quote! { #context_module }
            }
        };

        quote! {
            #error_impl
            #error_compat_impl
            #display_impl
            #context
        }
    }
}

impl GenericAwareNames for NamedStructInfo {
    fn name(&self) -> &syn::Ident {
        &self.field_container.name
    }

    fn generics(&self) -> &syn::Generics {
        &self.generics
    }
}

impl TupleStructInfo {
    fn generate_snafu(self) -> proc_macro2::TokenStream {
        let parameterized_struct_name = self.parameterized_name();

        let TupleStructInfo {
            crate_root,
            generics,
            name,
            transformation,
        } = self;

        let inner_type = transformation.ty();
        let transformation = transformation.transformation();

        let where_clauses: Vec<_> = generics
            .where_clause
            .iter()
            .flat_map(|c| c.predicates.iter().map(|p| quote! { #p }))
            .collect();

        let description_fn = quote! {
            fn description(&self) -> &str {
                #crate_root::Error::description(&self.0)
            }
        };

        let cause_fn = quote! {
            fn cause(&self) -> ::core::option::Option<&dyn #crate_root::Error> {
                #crate_root::Error::cause(&self.0)
            }
        };

        let source_fn = quote! {
            fn source(&self) -> ::core::option::Option<&(dyn #crate_root::Error + 'static)> {
                #crate_root::Error::source(&self.0)
            }
        };

        let backtrace_fn = quote! {
            fn backtrace(&self) -> ::core::option::Option<&#crate_root::Backtrace> {
                #crate_root::ErrorCompat::backtrace(&self.0)
            }
        };

        let std_backtrace_fn = if cfg!(feature = "unstable-backtraces-impl-std") {
            quote! {
                fn backtrace(&self) -> ::core::option::Option<&std::backtrace::Backtrace> {
                    #crate_root::ErrorCompat::backtrace(self)
                }
            }
        } else {
            quote! {}
        };

        let error_impl = quote! {
            #[allow(single_use_lifetimes)]
            impl#generics #crate_root::Error for #parameterized_struct_name
            where
                #(#where_clauses),*
            {
                #description_fn
                #cause_fn
                #source_fn
                #std_backtrace_fn
            }
        };

        let error_compat_impl = quote! {
            #[allow(single_use_lifetimes)]
            impl#generics #crate_root::ErrorCompat for #parameterized_struct_name
            where
                #(#where_clauses),*
            {
                #backtrace_fn
            }
        };

        let display_impl = quote! {
            #[allow(single_use_lifetimes)]
            impl#generics ::core::fmt::Display for #parameterized_struct_name
            where
                #(#where_clauses),*
            {
                fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                    ::core::fmt::Display::fmt(&self.0, f)
                }
            }
        };

        let from_impl = quote! {
            impl#generics ::core::convert::From<#inner_type> for #parameterized_struct_name
            where
                #(#where_clauses),*
            {
                fn from(other: #inner_type) -> Self {
                    #name((#transformation)(other))
                }
            }
        };

        quote! {
            #error_impl
            #error_compat_impl
            #display_impl
            #from_impl
        }
    }
}

impl GenericAwareNames for TupleStructInfo {
    fn name(&self) -> &syn::Ident {
        &self.name
    }

    fn generics(&self) -> &syn::Generics {
        &self.generics
    }
}

trait Transpose<T, E> {
    fn my_transpose(self) -> Result<Option<T>, E>;
}

impl<T, E> Transpose<T, E> for Option<Result<T, E>> {
    fn my_transpose(self) -> Result<Option<T>, E> {
        match self {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

mod sponge {
    use std::iter::FromIterator;

    pub struct AllErrors<T, E>(Result<T, Vec<E>>);

    impl<T, E> AllErrors<T, E> {
        pub fn into_result(self) -> Result<T, Vec<E>> {
            self.0
        }
    }

    impl<C, T, E> FromIterator<Result<C, E>> for AllErrors<T, E>
    where
        T: FromIterator<C>,
    {
        fn from_iter<I>(i: I) -> Self
        where
            I: IntoIterator<Item = Result<C, E>>,
        {
            let mut errors = Vec::new();

            let inner = i
                .into_iter()
                .flat_map(|v| match v {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        errors.push(e);
                        Err(())
                    }
                })
                .collect();

            if errors.is_empty() {
                AllErrors(Ok(inner))
            } else {
                AllErrors(Err(errors))
            }
        }
    }

    impl<C, T, E> FromIterator<Result<C, Vec<E>>> for AllErrors<T, E>
    where
        T: FromIterator<C>,
    {
        fn from_iter<I>(i: I) -> Self
        where
            I: IntoIterator<Item = Result<C, Vec<E>>>,
        {
            let mut errors = Vec::new();

            let inner = i
                .into_iter()
                .flat_map(|v| match v {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        errors.extend(e);
                        Err(())
                    }
                })
                .collect();

            if errors.is_empty() {
                AllErrors(Ok(inner))
            } else {
                AllErrors(Err(errors))
            }
        }
    }
}
