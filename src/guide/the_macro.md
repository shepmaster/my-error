# Details about the output of the `Snafu` macro

This procedural macro:

- produces the corresponding context selectors
- implements the [`Error`][Error] trait
- implements the [`Display`][Display] trait
- implements the [`ErrorCompat`][ErrorCompat] trait

## Detailed example

```rust
use snafu::{Backtrace, Snafu};
use std::path::PathBuf;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("Could not open config at {}: {}", filename.display(), source))]
    OpenConfig {
        filename: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("Could not open config: {}", "source"))]
    SaveConfig { source: std::io::Error },
    #[snafu(display("The user id {} is invalid", user_id))]
    UserIdInvalid { user_id: i32, backtrace: Backtrace },
}
```

### Generated code

<div style="background: #ffffd0; padding: 0.6em; margin-bottom: 0.6em;">

**Note** — The actual generated code may differ in exact names and
details. This section is only intended to provide general
guidance.

</div>

#### Context selectors

This will generate three additional types called *context
selectors*:

```rust,ignore
struct OpenConfig<P> { filename: P }
struct SaveConfig<P>;
struct UserIdInvalid<I> { user_id: I }
```

Notably:

1. One context selector is created for each enum variant.
1. The name of the selector is the same as the enum variant's name,
   unless a different name is specified using the `context` attribute.
1. The `source` and `backtrace` fields have been removed; the
   library will automatically handle this for you.
1. Each remaining field's type has been replaced with a generic
   type.
1. If there are no fields remaining for the user to specify, the
   selector will not require curly braces.

If the original variant had a `source` field, its context selector
will have an implementation of [`IntoError`][IntoError]:

```rust,ignore
impl<P> IntoError<Error> for OpenConfig<P>
where
    P: Into<PathBuf>,
```

Otherwise, the context selector will have an inherent method
`fail` and can be used with the [`ensure`](ensure) macro:

```rust,ignore
impl<I> UserIdInvalid<I>
where
    I: Into<i32>,
{
    fn fail<T>(self) -> Result<T, Error> { /* ... */ }
}
```

If the original variant had a `backtrace` field, the backtrace
will be automatically constructed when either `IntoError` or
`fail` are called.

#### `Error`

[`Error::source`][source] will return the underlying error, if
there is one:

```rust,ignore
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Error::OpenConfig { source, .. } => Some(source),
            Error::SaveConfig { source, .. } => Some(source),
            Error::UserIdInvalid { .. } => None,
        }
    }
}
```

[`Error::cause`][cause] will return the same as `source`. As
[`Error::description`][description] is soft-deprecated, it will
return a string matching the name of the variant.

#### `Display`

Every field of the enum variant is made available to the format
string, even if they are not used:

```rust,ignore
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter ) -> fmt::Result {
        match self {
            Error::OpenConfig { filename, source } =>
                write!(f, "Could not open config at {}: {}", filename.display(), source),
            Error::SaveConfig { source } =>
                write!(f, "Could not open config: {}", source),
            Error::UserIdInvalid { user_id, backtrace } =>
                write!(f, "The user id {} is invalid", user_id),
        }
    }
}
```

If no display format is specified, the variant's name will be used
by default. If the field is an underlying error, that error's
`Display` implementation will also be included.

#### `ErrorCompat`

Every variant that carries a backtrace will return a reference to
that backtrace.

```rust,ignore
impl snafu::ErrorCompat for Error {
    fn backtrace(&self) -> Option<&Backtrace> {
        match self {
            Error::OpenConfig { .. } => None,
            Error::SaveConfig { .. } => None,
            Error::UserIdInvalid { backtrace, .. } => Some(backtrace),
        }
    }
}
```

[Display]: std::fmt::Display
[ErrorCompat]: crate::ErrorCompat
[Error]: std::error::Error
[IntoError]: crate::IntoError
[cause]: std::error::Error::cause
[description]: std::error::Error::description
[source]: std::error::Error::source
