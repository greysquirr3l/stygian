//! Proc-macro crate for `#[derive(Extract)]`.
//!
//! Used by `stygian-browser` behind the `extract` feature flag.
//! Do not depend on this crate directly — use `stygian_browser::extract::Extract`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type, TypePath, parse_macro_input};

// ─── SelectorArgs ─────────────────────────────────────────────────────────────

/// Arguments parsed from `#[selector("css")]`, `#[selector("css", attr = "name")]`,
/// or `#[selector("css", nested)]`.
struct SelectorArgs {
    css: String,
    attr: Option<String>,
    nested: bool,
}

impl syn::parse::Parse for SelectorArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        // First positional argument must be the CSS selector string.
        let css: syn::LitStr = input.parse()?;
        let mut attr: Option<String> = None;
        let mut nested = false;

        while input.peek(syn::Token![,]) {
            let _: syn::Token![,] = input.parse()?;
            // Allow a trailing comma with nothing after it.
            if input.is_empty() {
                break;
            }
            let kw: syn::Ident = input.parse()?;
            if kw == "attr" {
                let _: syn::Token![=] = input.parse()?;
                let s: syn::LitStr = input.parse()?;
                attr = Some(s.value());
            } else if kw == "nested" {
                nested = true;
            } else {
                return Err(syn::Error::new_spanned(
                    kw,
                    "unknown selector option; expected `attr = \"...\"` or `nested`",
                ));
            }
        }

        Ok(SelectorArgs { css: css.value(), attr, nested })
    }
}

// ─── Helper: detect Option<T> ─────────────────────────────────────────────────

/// If `ty` is `Option<Inner>`, return `Some(&Inner)`.  Otherwise `None`.
fn unwrap_option(ty: &Type) -> Option<&Type> {
    let Type::Path(TypePath { qself: None, path }) = ty else {
        return None;
    };
    let seg = path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(ref args) = seg.arguments else {
        return None;
    };
    if let Some(GenericArgument::Type(inner)) = args.args.first() {
        Some(inner)
    } else {
        None
    }
}

// ─── #[derive(Extract)] ───────────────────────────────────────────────────────

/// Derive `stygian_browser::extract::Extractable` for a struct.
///
/// Each field must carry `#[selector("css")]`, `#[selector("css", attr = "name")]`,
/// or `#[selector("css", nested)]`.  Wrapping the field type in `Option<T>` makes
/// a missing element produce `None` instead of
/// `ExtractionError::Missing`.
#[proc_macro_derive(Extract, attributes(selector))]
pub fn derive_extract(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;

    // Reject non-struct inputs with a clean compile_error.
    let Data::Struct(ref data_struct) = input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Extract)] can only be applied to structs",
        ));
    };

    // Reject tuple / unit structs.
    let Fields::Named(ref named_fields) = data_struct.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(Extract)] requires a struct with named fields",
        ));
    };

    let mut field_assignments: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut field_idents: Vec<&syn::Ident> = Vec::new();

    for field in &named_fields.named {
        let field_name = field.ident.as_ref().expect("named field has ident");
        let field_name_str = field_name.to_string();

        // Require exactly one #[selector(...)] attribute per field.
        let Some(selector_attr) = field.attrs.iter().find(|a| a.path().is_ident("selector"))
        else {
            return Err(syn::Error::new_spanned(
                field,
                format!("field `{field_name_str}` is missing a #[selector(\"...\")] attribute"),
            ));
        };

        let args: SelectorArgs = selector_attr.parse_args()?;
        let css = &args.css;
        let is_optional = unwrap_option(&field.ty).is_some();

        let extraction = if args.nested {
            // #[selector("css", nested)] — delegate to T::extract_from
            let inner_ty = unwrap_option(&field.ty).unwrap_or(&field.ty);
            if is_optional {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        match __children.into_iter().next() {
                            None => None,
                            Some(ref __node) => Some(
                                <#inner_ty as ::stygian_browser::extract::Extractable>::extract_from(__node)
                                    .await
                                    .map_err(|__e| ::stygian_browser::extract::ExtractionError::Nested {
                                        field: #field_name_str,
                                        source: Box::new(__e),
                                    })?
                            ),
                        }
                    };
                }
            } else {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        let __first = __children.into_iter().next().ok_or(
                            ::stygian_browser::extract::ExtractionError::Missing {
                                field: #field_name_str,
                                selector: #css,
                            },
                        )?;
                        <#inner_ty as ::stygian_browser::extract::Extractable>::extract_from(&__first)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::Nested {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?
                    };
                }
            }
        } else if let Some(attr) = &args.attr {
            // #[selector("css", attr = "name")] — extract attribute value
            if is_optional {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        match __children.into_iter().next() {
                            None => None,
                            Some(ref __node) => __node.attr(#attr)
                                .await
                                .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                    field: #field_name_str,
                                    source: Box::new(__e),
                                })?,
                        }
                    };
                }
            } else {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        let __first = __children.into_iter().next().ok_or(
                            ::stygian_browser::extract::ExtractionError::Missing {
                                field: #field_name_str,
                                selector: #css,
                            },
                        )?;
                        __first.attr(#attr)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?
                            .unwrap_or_default()
                    };
                }
            }
        } else {
            // #[selector("css")] — extract text_content()
            if is_optional {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        match __children.into_iter().next() {
                            None => None,
                            Some(ref __node) => Some(
                                __node.text_content()
                                    .await
                                    .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                        field: #field_name_str,
                                        source: Box::new(__e),
                                    })?
                            ),
                        }
                    };
                }
            } else {
                quote! {
                    let #field_name = {
                        let __children = node.children_matching(#css)
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?;
                        let __first = __children.into_iter().next().ok_or(
                            ::stygian_browser::extract::ExtractionError::Missing {
                                field: #field_name_str,
                                selector: #css,
                            },
                        )?;
                        __first.text_content()
                            .await
                            .map_err(|__e| ::stygian_browser::extract::ExtractionError::CdpFailed {
                                field: #field_name_str,
                                source: Box::new(__e),
                            })?
                    };
                }
            }
        };

        field_assignments.push(extraction);
        field_idents.push(field_name);
    }

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::stygian_browser::extract::Extractable for #name #ty_generics
        #where_clause
        {
            async fn extract_from(
                node: &::stygian_browser::page::NodeHandle,
            ) -> ::std::result::Result<Self, ::stygian_browser::extract::ExtractionError> {
                #(#field_assignments)*
                Ok(Self { #(#field_idents),* })
            }
        }
    })
}
