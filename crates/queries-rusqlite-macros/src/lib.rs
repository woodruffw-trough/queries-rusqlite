//! Implementation of the macros re-exported by `queries-rusqlite`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, FnArg, ItemTrait, Meta, Pat, Path, ReturnType,
    TraitItem, TraitItemFn, Type, ext::IdentExt, parse::Parser, parse_quote,
    punctuated::Punctuated,
};

const GENERATED_METHODS: &[&str] = &[
    "from_conn",
    "from_conn_mut",
    "from_connection",
    "from_tx",
    "begin",
    "commit",
    "rollback",
];

/// Replace a trait with a struct of synchronous query methods.
///
/// The struct keeps the trait's name, visibility, and documentation. Each method
/// takes `&self` and returns `rusqlite::Result<T>`, where `T` is its declared
/// return type. Method documentation is preserved.
///
/// # Declarations
///
/// Each method needs `#[query = SQL]`, where `SQL` is an expression yielding
/// `&str`, such as a string literal or `include_str!("query.sql")`. Arguments
/// must implement `rusqlite::ToSql`. They bind in declaration order to `?1`,
/// `?2`, and so on. SQL and column types are checked at runtime.
///
/// Traits cannot be unsafe, auto, generic, or have bounds. They may contain only
/// methods without receivers, bodies, generics, or qualifiers such as `async`.
/// Arguments must have simple names. The generated method names listed below
/// are reserved. Only `doc`, `cfg`, and `cfg_attr` attributes are preserved.
///
/// # Results
///
/// | Declared type | Result |
/// | --- | --- |
/// | `T: FromRow` | Exactly one row; zero or multiple rows are errors. |
/// | `Option<T>` | Zero or one row; multiple rows are an error. |
/// | `Vec<T>` | All rows, stopping at the first error. |
/// | `Query<'_, T>` | A prepared query for lazy iteration. |
/// | `()` or omitted | Execute without result rows; discard the affected row count. |
///
/// Row types implement `queries_rusqlite::FromRow`. Use `(T,)` for a single
/// column. Return type aliases are supported. Preparation, binding, execution,
/// and decoding errors propagate from rusqlite; lazy queries return execution
/// and decoding errors as iterator items.
///
/// # Connections and transactions
///
/// - `from_conn(&connection)` borrows a connection for queries.
/// - `from_conn_mut(&mut connection)` borrows a connection and supports `begin()`.
/// - `from_connection(connection)` owns a connection and supports `begin()`.
/// - `from_tx(transaction)` owns a transaction and supports `commit()` and `rollback()`.
///
/// `begin()` borrows the wrapper mutably and returns a transaction wrapper with
/// the same query methods. `commit()` and `rollback()` consume that wrapper.
/// Dropping it follows the transaction's drop behavior, which defaults to rollback.
/// To configure a transaction, create it with rusqlite and pass it to `from_tx`.
///
/// # Crate names
///
/// Use `#[queries(crate = path)]` if `queries-rusqlite` is renamed. Callers must
/// also declare a dependency named `rusqlite`.
///
/// # Invalid declarations
///
/// Every method must declare its SQL:
///
/// ```compile_fail
/// #[queries_rusqlite_macros::queries]
/// trait MissingSql {
///     fn count() -> (i64,);
/// }
/// ```
///
/// Async methods and receivers are not supported:
///
/// ```compile_fail
/// #[queries_rusqlite_macros::queries]
/// trait AsyncQuery {
///     #[query = "SELECT 1"]
///     async fn count(&self) -> (i64,);
/// }
/// ```
#[proc_macro_attribute]
pub fn queries(attributes: TokenStream, input: TokenStream) -> TokenStream {
    finish(expand_queries(attributes.into(), input.into())).into()
}

/// Derive `queries_rusqlite::FromRow` for a named or tuple struct.
///
/// Named fields use column names; tuple fields use zero-based column positions.
/// Each field must implement `rusqlite::types::FromSql`. Generic structs are
/// supported; the derive adds this bound for each field type. Missing columns
/// and conversion errors propagate from `rusqlite::Row::get`.
///
/// Use `#[column = "name"]` to read a field by that column name and
/// `#[from_row(crate = path)]` to select a renamed `queries-rusqlite` crate.
/// Callers must also declare a dependency named `rusqlite`.
/// Unit structs, enums, and unions are not supported:
///
/// ```compile_fail
/// #[derive(queries_rusqlite_macros::FromRow)]
/// enum NotARow {
///     Empty,
/// }
/// ```
#[proc_macro_derive(FromRow, attributes(from_row, column))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    finish(expand_from_row(input.into())).into()
}

fn finish(result: syn::Result<Tokens>) -> Tokens {
    result.unwrap_or_else(syn::Error::into_compile_error)
}

fn crate_path(tokens: Tokens) -> syn::Result<Path> {
    let attributes = Punctuated::<Meta, syn::Token![,]>::parse_terminated.parse2(tokens)?;
    let mut selected = None;
    for attribute in attributes {
        match attribute {
            Meta::NameValue(value) if value.path.is_ident("crate") && selected.is_none() => {
                if let Expr::Path(path) = &value.value
                    && path.qself.is_none()
                {
                    selected = Some(path.path.clone());
                    continue;
                }
                return Err(syn::Error::new_spanned(value, "expected a crate path"));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "expected a single `crate = path` argument",
                ));
            }
        }
    }
    Ok(selected.unwrap_or_else(|| parse_quote!(::queries_rusqlite)))
}

fn expand_queries(attributes: Tokens, input: Tokens) -> syn::Result<Tokens> {
    let path = crate_path(attributes)?;
    let item: ItemTrait = syn::parse2(input)?;
    if item.unsafety.is_some()
        || item.modifiers.require_empty().is_err()
        || !item.generics.params.is_empty()
        || item.generics.where_clause.is_some()
        || !item.supertraits.is_empty()
    {
        return Err(syn::Error::new_spanned(
            &item,
            "query traits cannot be unsafe, auto, generic, or have bounds",
        ));
    }
    check_attributes(&item.attrs)?;
    let mut methods = Vec::new();
    for member in &item.items {
        let TraitItem::Fn(method) = member else {
            return Err(syn::Error::new_spanned(member, "expected a query method"));
        };
        methods.push(expand_method(method, &path)?);
    }
    let name = &item.ident;
    let visibility = &item.vis;
    let attributes = &item.attrs;
    // Apply cfg attributes to every item, so a disabled wrapper leaves no impls.
    let conditions = item.attrs.iter().filter(|attribute| {
        attribute.path().is_ident("cfg") || attribute.path().is_ident("cfg_attr")
    });
    let conditions: Vec<_> = conditions.collect();
    Ok(quote! {
        #(#attributes)*
        #visibility struct #name<__QueriesConnection> {
            connection: __QueriesConnection,
        }

        #(#conditions)*
        impl<__QueriesConnection: #path::__private::ConnectionSource> #name<__QueriesConnection> {
            #(#methods)*
        }

        #(#conditions)*
        impl<'conn> #name<&'conn ::rusqlite::Connection> {
            /// Borrow a connection for queries.
            #[inline]
            pub fn from_conn(connection: &'conn ::rusqlite::Connection) -> Self {
                Self { connection }
            }
        }

        #(#conditions)*
        impl<'conn> #name<&'conn mut ::rusqlite::Connection> {
            /// Borrow a connection for queries and transactions.
            #[inline]
            pub fn from_conn_mut(connection: &'conn mut ::rusqlite::Connection) -> Self {
                Self { connection }
            }

            /// Begin a transaction using the connection's configured behavior.
            ///
            /// Return an error if SQLite cannot start the transaction.
            #[inline]
            pub fn begin(&mut self) -> ::rusqlite::Result<#name<::rusqlite::Transaction<'_>>> {
                self.connection.transaction().map(#name::from_tx)
            }
        }

        #(#conditions)*
        impl #name<::rusqlite::Connection> {
            /// Take ownership of a connection.
            #[inline]
            pub fn from_connection(connection: ::rusqlite::Connection) -> Self {
                Self { connection }
            }

            /// Begin a transaction using the connection's configured behavior.
            ///
            /// Return an error if SQLite cannot start the transaction.
            #[inline]
            pub fn begin(&mut self) -> ::rusqlite::Result<#name<::rusqlite::Transaction<'_>>> {
                self.connection.transaction().map(#name::from_tx)
            }
        }

        #(#conditions)*
        impl<'conn> #name<::rusqlite::Transaction<'conn>> {
            /// Take ownership of a transaction, preserving its drop behavior.
            #[inline]
            pub fn from_tx(connection: ::rusqlite::Transaction<'conn>) -> Self {
                Self { connection }
            }

            /// Consume the wrapper and commit the transaction.
            ///
            /// Return any error from `rusqlite::Transaction::commit`.
            #[inline]
            pub fn commit(self) -> ::rusqlite::Result<()> {
                self.connection.commit()
            }

            /// Consume the wrapper and roll back the transaction.
            ///
            /// Return any error from `rusqlite::Transaction::rollback`.
            #[inline]
            pub fn rollback(self) -> ::rusqlite::Result<()> {
                self.connection.rollback()
            }
        }
    })
}

fn check_attributes(attributes: &[Attribute]) -> syn::Result<()> {
    for attribute in attributes {
        if !["doc", "cfg", "cfg_attr"]
            .iter()
            .any(|name| attribute.path().is_ident(name))
        {
            return Err(syn::Error::new_spanned(
                attribute,
                "unsupported query attribute",
            ));
        }
    }
    Ok(())
}

fn expand_method(method: &TraitItemFn, path: &Path) -> syn::Result<Tokens> {
    let signature = &method.sig;
    if signature.constness.is_some()
        || signature.asyncness.is_some()
        || !matches!(signature.safety, syn::Safety::Default)
        || signature.abi.is_some()
        || signature.variadic.is_some()
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
        || method.default.is_some()
    {
        return Err(syn::Error::new_spanned(
            method,
            "query methods must be synchronous declarations without qualifiers, generics, or bodies",
        ));
    }
    let name = &signature.ident;
    let method_name = name.unraw().to_string();
    if GENERATED_METHODS.contains(&method_name.as_str()) {
        return Err(syn::Error::new_spanned(
            name,
            "this name is reserved for a generated method",
        ));
    }
    let mut sql = None;
    let mut attributes = Vec::new();
    for attribute in &method.attrs {
        if attribute.path().is_ident("query") {
            if sql.is_some() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "duplicate query attribute",
                ));
            }
            let Meta::NameValue(value) = &attribute.meta else {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "expected `#[query = SQL]`",
                ));
            };
            sql = Some(&value.value);
        } else {
            attributes.push(attribute.clone());
        }
    }
    check_attributes(&attributes)?;
    let sql =
        sql.ok_or_else(|| syn::Error::new_spanned(name, "missing `#[query = SQL]` attribute"))?;
    let mut arguments = Vec::new();
    for argument in &signature.inputs {
        let FnArg::Typed(argument) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "query declarations must not have a receiver",
            ));
        };
        if let Some(attribute) = argument.attrs.first() {
            return Err(syn::Error::new_spanned(
                attribute,
                "query arguments cannot have attributes",
            ));
        }
        let Pat::Ident(pattern) = &*argument.pat else {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "query arguments must have simple names",
            ));
        };
        if pattern.by_ref.is_some() || pattern.subpat.is_some() {
            return Err(syn::Error::new_spanned(
                pattern,
                "query arguments must have simple names",
            ));
        }
        arguments.push(&pattern.ident);
    }
    let output: Type = match &signature.output {
        ReturnType::Default => parse_quote!(()),
        ReturnType::Type(_, output) => (**output).clone(),
    };
    let inputs = &signature.inputs;
    Ok(quote! {
        #(#attributes)*
        #[inline]
        pub fn #name(&self, #inputs) -> ::rusqlite::Result<#output> {
            use #path::__private::SingleRowFallback as _;
            <#output as #path::__private::FromRows<'_, {
                #path::__private::FromRowsCategory::<#output>::VALUE
            }>>::from_rows(
                #path::__private::ConnectionSource::connection(&self.connection),
                #sql,
                ::rusqlite::params![#(#arguments),*],
            )
        }
    })
}

fn expand_from_row(input: Tokens) -> syn::Result<Tokens> {
    let item: DeriveInput = syn::parse2(input)?;
    let mut override_path = None;
    for attribute in &item.attrs {
        if attribute.path().is_ident("column") {
            return Err(syn::Error::new_spanned(
                attribute,
                "column attributes belong on fields",
            ));
        }
        if attribute.path().is_ident("from_row") {
            if override_path.is_some() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "duplicate from_row attribute",
                ));
            }
            override_path = Some(crate_path(attribute.parse_args()?)?);
        }
    }
    let path = override_path.unwrap_or_else(|| parse_quote!(::queries_rusqlite));
    let Data::Struct(data) = &item.data else {
        return Err(syn::Error::new_spanned(&item, "FromRow requires a struct"));
    };
    if matches!(data.fields, Fields::Unit) {
        return Err(syn::Error::new_spanned(
            &item,
            "FromRow requires a named or tuple struct",
        ));
    }
    let mut generics = item.generics.clone();
    let mut initializers = Vec::new();
    for (index, field) in data.fields.iter().enumerate() {
        let ty = &field.ty;
        generics
            .make_where_clause()
            .predicates
            .push(parse_quote!(#ty: ::rusqlite::types::FromSql));
        let mut column = None;
        for attribute in &field.attrs {
            if attribute.path().is_ident("from_row") {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "from_row attributes belong on the struct",
                ));
            }
            if attribute.path().is_ident("column") {
                if column.is_some() {
                    return Err(syn::Error::new_spanned(
                        attribute,
                        "duplicate column attribute",
                    ));
                }
                let Meta::NameValue(value) = &attribute.meta else {
                    return Err(syn::Error::new_spanned(
                        attribute,
                        "expected `#[column = \"name\"]`",
                    ));
                };
                let Expr::Lit(value) = &value.value else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "expected a column name string",
                    ));
                };
                let syn::Lit::Str(value) = &value.lit else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "expected a column name string",
                    ));
                };
                column = Some(quote!(#value));
            }
        }
        if let Some(name) = &field.ident {
            let column_name = name.unraw().to_string();
            let column = column.unwrap_or_else(|| quote!(#column_name));
            initializers.push(quote!(#name: row.get(#column)?));
        } else {
            let column = column.unwrap_or_else(|| quote!(#index));
            initializers.push(quote!(row.get(#column)?));
        }
    }
    let initializer = match &data.fields {
        Fields::Named(_) => quote!(Self { #(#initializers),* }),
        _ => quote!(Self(#(#initializers),*)),
    };
    let name = &item.ident;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics #path::FromRow for #name #type_generics #where_clause {
            #[inline]
            fn from_row(row: &::rusqlite::Row<'_>) -> ::rusqlite::Result<Self> {
                ::core::result::Result::Ok(#initializer)
            }
        }
    })
}

#[cfg(test)]
mod tests;
