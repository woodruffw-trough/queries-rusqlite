use super::*;

fn query_error(input: Tokens, expected: &str) {
    let error = expand_queries(Tokens::new(), input).unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
}

fn row_error(input: Tokens, expected: &str) {
    let error = expand_from_row(input).unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
}

#[test]
fn finish_preserves_output_and_reports_errors() {
    assert_eq!(finish(Ok(quote!(example))).to_string(), "example");
    assert!(
        finish(Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "bad input"
        )))
        .to_string()
        .contains("compile_error")
    );
}

#[test]
fn crate_paths() {
    assert_eq!(quote!(::queries_rusqlite).to_string(), {
        let path = crate_path(Tokens::new()).unwrap();
        quote!(#path).to_string()
    });
    for tokens in [quote!(crate = renamed), quote!(crate = ::renamed,)] {
        assert_eq!(crate_path(tokens).unwrap().segments[0].ident, "renamed");
    }
    for tokens in [
        quote!(other = renamed),
        quote!(crate),
        quote!(crate(renamed)),
        quote!(crate = renamed, crate = twice),
    ] {
        assert!(
            crate_path(tokens)
                .err()
                .unwrap()
                .to_string()
                .contains("single")
        );
    }
    for tokens in [
        quote!(crate = "renamed"),
        quote!(crate = <T as Trait>::Name),
    ] {
        assert!(
            crate_path(tokens)
                .err()
                .unwrap()
                .to_string()
                .contains("crate path")
        );
    }
    assert!(crate_path(quote!(=)).is_err());
}

#[test]
fn query_expansion_preserves_visibility_docs_and_conditions() {
    let output = expand_queries(
        quote!(crate = renamed),
        quote! {
            #[doc = "Read users."]
            #[cfg(feature = "users")]
            #[cfg_attr(feature = "extra", doc = "Extra docs.")]
            pub trait Users {
                #[doc = "Read one user."]
                #[cfg(feature = "one")]
                #[query = include_str!("one.sql")]
                fn one(mut r#type: i64, name: &str) -> Option<User>;
                #[query = SQL]
                fn execute();
            }
        },
    )
    .unwrap();
    let file: syn::File = syn::parse2(output.clone()).unwrap();
    assert_eq!(file.items.len(), 6);
    let output = output.to_string();
    for expected in [
        "pub struct Users",
        "Read users.",
        "Read one user.",
        "pub fn one",
        "include_str ! (\"one.sql\")",
        "params ! [r#type , name]",
        "Result < Option < User > >",
        "Result < () >",
        "from_conn",
        "from_conn_mut",
        "from_connection",
        "from_tx",
        "fn begin",
        "fn commit",
        "fn rollback",
    ] {
        assert!(output.contains(expected), "missing {expected} in {output}");
    }
    assert_eq!(output.matches("cfg (feature = \"users\")").count(), 6);
    assert_eq!(output.matches("cfg_attr (feature = \"extra\"").count(), 6);
    assert!(!output.contains("queries_rusqlite"));
    assert!(output.contains(":: rusqlite :: Connection"));
    assert!(output.contains(":: rusqlite :: Result"));
    assert!(output.contains(":: rusqlite :: params !"));
    assert!(!output.contains("renamed :: rusqlite"));
}

#[test]
fn empty_query_trait_is_valid() {
    let output = expand_queries(
        Tokens::new(),
        quote!(
            trait Empty {}
        ),
    )
    .unwrap();
    assert!(output.to_string().contains(":: queries_rusqlite"));
}

#[test]
fn invalid_query_trait_shapes() {
    for input in [
        quote!(
            unsafe trait Invalid {}
        ),
        quote!(auto trait Invalid {}),
        quote!(
            trait Invalid<T> {}
        ),
        quote!(
            trait Invalid
            where
                Self: Sized,
            {
            }
        ),
        quote!(
            trait Invalid: Sized {}
        ),
    ] {
        query_error(input, "query traits cannot");
    }
    for input in [
        quote!(
            trait Invalid {
                type Item;
            }
        ),
        quote!(
            trait Invalid {
                const ITEM: i32;
            }
        ),
    ] {
        query_error(input, "expected a query method");
    }
    query_error(
        quote!(
            struct Invalid;
        ),
        "trait",
    );
    query_error(
        quote!(
            #[allow(dead_code)]
            trait Invalid {}
        ),
        "unsupported query attribute",
    );
    assert!(
        expand_queries(
            quote!(invalid),
            quote!(
                trait Empty {}
            )
        )
        .is_err()
    );
}

#[test]
fn invalid_query_signatures() {
    for input in [
        quote!(
            trait Invalid {
                const fn bad();
            }
        ),
        quote!(
            trait Invalid {
                async fn bad();
            }
        ),
        quote!(
            trait Invalid {
                unsafe fn bad();
            }
        ),
        quote!(
            trait Invalid {
                extern "C" fn bad();
            }
        ),
        quote!(
            trait Invalid {
                fn bad(...);
            }
        ),
        quote!(
            trait Invalid {
                fn bad<T>();
            }
        ),
        quote!(
            trait Invalid {
                fn bad()
                where
                    Self: Sized;
            }
        ),
        quote!(
            trait Invalid {
                fn bad() {}
            }
        ),
    ] {
        query_error(input, "synchronous declarations");
    }
    query_error(
        quote!(
            trait Invalid {
                #[query = "SELECT 1"]
                safe fn bad();
            }
        ),
        "expected",
    );
    for name in [
        "from_conn",
        "from_conn_mut",
        "from_connection",
        "from_tx",
        "begin",
        "commit",
        "rollback",
    ] {
        let name = syn::Ident::new(name, proc_macro2::Span::call_site());
        query_error(quote!(trait Invalid { fn #name(); }), "reserved");
    }
    query_error(
        quote!(
            trait Invalid {
                fn r#begin();
            }
        ),
        "reserved",
    );
    query_error(
        quote!(
            trait Invalid {
                #[query = ""]
                fn bad(&self);
            }
        ),
        "receiver",
    );
    for argument in [
        quote!((left, right): (i32, i32)),
        quote!(ref value: i32),
        quote!(value @ _: i32),
    ] {
        query_error(
            quote!(trait Invalid { #[query = ""] fn bad(#argument); }),
            "simple names",
        );
    }
}

#[test]
fn invalid_query_attributes() {
    query_error(
        quote!(
            trait Invalid {
                #[query = "SELECT 1"]
                fn bad(#[cfg(any())] ignored: i64);
            }
        ),
        "query arguments cannot have attributes",
    );
    query_error(
        quote!(
            trait Invalid {
                fn bad();
            }
        ),
        "missing",
    );
    query_error(
        quote!(
            trait Invalid {
                #[query = ""]
                #[query = ""]
                fn bad();
            }
        ),
        "duplicate",
    );
    query_error(
        quote!(
            trait Invalid {
                #[query("")]
                fn bad();
            }
        ),
        "expected `#[query = SQL]`",
    );
    query_error(
        quote!(
            trait Invalid {
                #[query = ""]
                #[inline]
                fn bad();
            }
        ),
        "unsupported query attribute",
    );
}

#[test]
fn named_from_row_expansion() {
    let output = expand_from_row(quote! {
        #[from_row(crate = renamed)]
        #[doc = "A row."]
        struct Row<T> where T: Clone {
            #[doc = "Its identifier."]
            id: i64,
            r#type: T,
            #[column = "different"]
            name: String,
        }
    })
    .unwrap();
    let _parsed: syn::ItemImpl = syn::parse2(output.clone()).unwrap();
    let output = output.to_string();
    for expected in [
        "impl < T > renamed :: FromRow for Row < T >",
        "T : Clone",
        "T : :: rusqlite :: types :: FromSql",
        "id : row . get (\"id\") ?",
        "r#type : row . get (\"type\") ?",
        "name : row . get (\"different\") ?",
        ":: core :: result :: Result :: Ok",
        "automatically_derived",
    ] {
        assert!(output.contains(expected), "missing {expected} in {output}");
    }
}

#[test]
fn tuple_and_empty_from_row_expansion() {
    let output = expand_from_row(quote! {
        struct Row(i64, #[column = "label"] String);
    })
    .unwrap()
    .to_string();
    assert!(output.contains("row . get (0usize) ?"));
    assert!(output.contains("row . get (\"label\") ?"));
    for input in [
        quote!(
            struct Empty {}
        ),
        quote!(
            struct Empty();
        ),
    ] {
        let _parsed: syn::ItemImpl = syn::parse2(expand_from_row(input).unwrap()).unwrap();
    }
}

#[test]
fn invalid_from_row_shapes_and_attributes() {
    row_error(
        quote!(
            #[column = "misplaced"]
            struct Invalid(i32);
        ),
        "column attributes belong on fields",
    );
    row_error(
        quote!(
            struct Invalid(#[from_row(crate = renamed)] i32);
        ),
        "from_row attributes belong on the struct",
    );
    row_error(
        quote!(
            fn invalid() {}
        ),
        "one of",
    );
    row_error(
        quote!(
            enum Invalid {
                A,
            }
        ),
        "requires a struct",
    );
    row_error(quote!(union Invalid { field: i32 }), "requires a struct");
    row_error(
        quote!(
            struct Invalid;
        ),
        "named or tuple struct",
    );
    row_error(
        quote!(
            #[from_row(crate = first)]
            #[from_row(crate = second)]
            struct Invalid(i32);
        ),
        "duplicate from_row",
    );
    row_error(
        quote!(
            #[from_row]
            struct Invalid(i32);
        ),
        "arguments",
    );
    row_error(
        quote!(
            #[from_row(unknown)]
            struct Invalid(i32);
        ),
        "single",
    );
    row_error(
        quote!(
            struct Invalid(
                #[column = "first"]
                #[column = "second"]
                i32,
            );
        ),
        "duplicate column",
    );
    row_error(
        quote!(
            struct Invalid(#[column("first")] i32);
        ),
        "expected `#[column",
    );
    row_error(
        quote!(
            struct Invalid(#[column = COLUMN] i32);
        ),
        "column name string",
    );
    row_error(
        quote!(
            struct Invalid(#[column = 1] i32);
        ),
        "column name string",
    );
}
