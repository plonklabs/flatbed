//! Implementation of the #[flatbed::main] macro

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Ident, ItemFn, LitStr, Token, Type,
};

/// Parsed attributes for the #[flatbed::main] macro
struct MainAttrs {
    bind: LitStr,
    context: Option<Type>,
    init: Option<Ident>,
}

impl Parse for MainAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut bind: Option<LitStr> = None;
        let mut context: Option<Type> = None;
        let mut init: Option<Ident> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "bind" => {
                    bind = Some(input.parse()?);
                }
                "context" => {
                    context = Some(input.parse()?);
                }
                "init" => {
                    init = Some(input.parse()?);
                }
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("Unknown attribute: {}. Expected: bind, context, init", key),
                    ))
                }
            }

            // Consume trailing comma if present
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let Some(bind) = bind else {
            return Err(syn::Error::new(
                input.span(),
                "Missing required attribute: bind (e.g., bind = \"0.0.0.0:8080\")",
            ));
        };

        Ok(MainAttrs {
            bind,
            context,
            init,
        })
    }
}

pub fn main_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as MainAttrs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let bind_addr = attrs.bind;
    let fn_name = &input_fn.sig.ident;
    let fn_block = &input_fn.block;
    let fn_attrs = &input_fn.attrs;

    // Verify function is async
    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(&input_fn.sig, "#[flatbed::main] function must be async")
            .to_compile_error()
            .into();
    }

    // Verify function name is main
    if fn_name != "main" {
        return syn::Error::new_spanned(
            fn_name,
            "#[flatbed::main] can only be applied to fn main()",
        )
        .to_compile_error()
        .into();
    }

    // Generate context initialization code
    let (_context_type, context_init) = match (&attrs.context, &attrs.init) {
        (Some(ctx_type), Some(init_fn)) => {
            // User provided context type and init function
            (
                quote! { #ctx_type },
                quote! {
                    let app_context = match #init_fn().await {
                        Ok(ctx) => ctx,
                        Err(e) => {
                            eprintln!("Failed to initialize context: {}", e);
                            std::process::exit(1);
                        }
                    };
                },
            )
        }
        (Some(ctx_type), None) => {
            // Context type but no init - use Default
            (
                quote! { #ctx_type },
                quote! {
                    let app_context = <#ctx_type as Default>::default();
                },
            )
        }
        (None, Some(_)) => {
            return syn::Error::new_spanned(
                &input_fn.sig,
                "init requires context type. Add: context = YourContextType",
            )
            .to_compile_error()
            .into();
        }
        (None, None) => {
            // No context - use unit type
            (
                quote! { () },
                quote! {
                    let app_context = ();
                },
            )
        }
    };

    let expanded = quote! {
        #(#fn_attrs)*
        fn #fn_name() {
            // Build the tokio runtime
            let runtime = ::flatbed::tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");

            runtime.block_on(async {
                // Validate routes at startup
                if let Err(conflict) = ::flatbed::validate_routes() {
                    eprintln!("Route conflict detected: {}", conflict);
                    std::process::exit(1);
                }

                // Initialize context
                #context_init

                // Build router from inventory-registered routes
                let router = ::flatbed::hyper::build_router(|route_info| {
                    // This closure maps RouteInfo to the generated hyper handler
                    // We use a match on path to find the correct handler
                    // This is generated at compile time by inventory
                    |parts, body, ct| {
                        // Default handler that returns 500
                        // The actual handlers are registered via inventory
                        Box::pin(async move {
                            Err(::flatbed::Error::Custom("Handler not found".into()))
                        })
                    }
                });

                // Create flatbed config
                let config = ::flatbed::FlatbedConfig::default();

                // Parse bind address
                let bind_addr: std::net::SocketAddr = #bind_addr
                    .parse()
                    .expect("Invalid bind address");

                // Run user's async block first
                #fn_block

                // Start the server
                let server = ::flatbed::hyper::AutoServer::new(
                    bind_addr,
                    router,
                    app_context,
                    config,
                );

                if let Err(e) = server.serve().await {
                    eprintln!("Server error: {}", e);
                    std::process::exit(1);
                }
            });
        }
    };

    TokenStream::from(expanded)
}
