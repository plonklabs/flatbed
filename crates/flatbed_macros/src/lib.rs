use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, GenericArgument, Ident, ItemFn, LitBool, LitStr, PathArguments, Token, Type,
};

mod main_macro;

/// HTTP methods supported by the route macro
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "DELETE" => Some(HttpMethod::Delete),
            "PATCH" => Some(HttpMethod::Patch),
            "HEAD" => Some(HttpMethod::Head),
            "OPTIONS" => Some(HttpMethod::Options),
            _ => None,
        }
    }

    fn to_flatbed_token(self) -> proc_macro2::TokenStream {
        match self {
            HttpMethod::Get => quote! { ::flatbed::HttpMethod::Get },
            HttpMethod::Post => quote! { ::flatbed::HttpMethod::Post },
            HttpMethod::Put => quote! { ::flatbed::HttpMethod::Put },
            HttpMethod::Delete => quote! { ::flatbed::HttpMethod::Delete },
            HttpMethod::Patch => quote! { ::flatbed::HttpMethod::Patch },
            HttpMethod::Head => quote! { ::flatbed::HttpMethod::Head },
            HttpMethod::Options => quote! { ::flatbed::HttpMethod::Options },
        }
    }
}

/// Parsed attributes for the #[route] macro
struct RouteAttrs {
    path: LitStr,
    method: HttpMethod,
    version: Option<LitStr>,
    tag: Option<LitStr>,
    summary: Option<LitStr>,
    operation_id: Option<LitStr>,
    deprecated: bool,
}

impl Parse for RouteAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Parse the required path argument first
        let path: LitStr = input.parse()?;

        let mut attrs = RouteAttrs {
            path,
            method: HttpMethod::Post, // Default to POST
            version: None,
            tag: None,
            summary: None,
            operation_id: None,
            deprecated: false,
        };

        // Parse optional key = value attributes
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;

            if input.is_empty() {
                break;
            }

            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "method" => {
                    let method_str: LitStr = input.parse()?;
                    attrs.method = HttpMethod::from_str(&method_str.value()).ok_or_else(|| {
                        syn::Error::new(
                            method_str.span(),
                            "Invalid HTTP method. Use: GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS",
                        )
                    })?;
                }
                "version" => {
                    attrs.version = Some(input.parse()?);
                }
                "tag" => {
                    attrs.tag = Some(input.parse()?);
                }
                "summary" => {
                    attrs.summary = Some(input.parse()?);
                }
                "operation_id" => {
                    attrs.operation_id = Some(input.parse()?);
                }
                "deprecated" => {
                    let val: LitBool = input.parse()?;
                    attrs.deprecated = val.value();
                }
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("Unknown attribute: {}", key),
                    ))
                }
            }
        }

        Ok(attrs)
    }
}

/// Helper to extract the type name without module path and lifetime parameters
fn extract_type_name(ty: &Type) -> String {
    let full = ty.to_token_stream().to_string();
    // Extract just the type name (last segment of path), removing any lifetime params
    let name = full.split("::").last().unwrap_or(&full).trim();
    // Remove lifetime parameters like <'a> or <'_>
    if let Some(idx) = name.find('<') {
        name[..idx].trim().to_string()
    } else {
        name.to_string()
    }
}

/// Extract the inner type from Arc<C>
fn extract_arc_inner_type(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let segment = type_path.path.segments.last()?;
    if segment.ident != "Arc" {
        return None;
    }

    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };

    let GenericArgument::Type(inner_type) = args.args.first()? else {
        return None;
    };

    Some(inner_type.clone())
}

/// Result of parsing Request<T, C> type
struct RequestTypeInfo {
    /// The body type T
    body_type: Type,
    /// The context type C (None if Request<T> without context)
    context_type: Option<Type>,
    /// Whether context is unit type ()
    has_context: bool,
}

/// Parse Request<T> or Request<T, C> type and extract inner types
fn parse_request_type(ty: &Type) -> Option<RequestTypeInfo> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let segment = type_path.path.segments.last()?;
    if segment.ident != "Request" {
        return None;
    }

    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };

    let mut iter = args.args.iter();

    // First type parameter is the body type T
    let GenericArgument::Type(body_type) = iter.next()? else {
        return None;
    };

    // Second type parameter (if present) is the context type C
    let context_type = iter.next().and_then(|arg| {
        if let GenericArgument::Type(t) = arg {
            Some(t.clone())
        } else {
            None
        }
    });

    // Check if context is unit type ()
    let has_context = context_type
        .as_ref()
        .map(|t| {
            let s = t.to_token_stream().to_string();
            s != "()" && !s.is_empty()
        })
        .unwrap_or(false);

    Some(RequestTypeInfo {
        body_type: body_type.clone(),
        context_type,
        has_context,
    })
}

/// Result of parsing Result<Response<T>, FlatbedError<D>>
struct ResponseTypeInfo {
    /// The response body type T
    body_type: Type,
    /// String representation for metadata
    body_type_str: String,
}

/// Parse Result<Response<T>, FlatbedError<D>> and extract the response body type
fn parse_response_type(ty: &Type) -> Option<ResponseTypeInfo> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let result_seg = type_path.path.segments.last()?;
    if result_seg.ident != "Result" {
        return None;
    }

    let PathArguments::AngleBracketed(result_args) = &result_seg.arguments else {
        return None;
    };

    // Get first arg - should be Response<T>
    let GenericArgument::Type(response_wrapper) = result_args.args.first()? else {
        return None;
    };

    // Parse Response<T>
    let Type::Path(response_path) = response_wrapper else {
        return None;
    };

    let response_seg = response_path.path.segments.last()?;
    if response_seg.ident != "Response" {
        return None;
    }

    let PathArguments::AngleBracketed(response_args) = &response_seg.arguments else {
        return None;
    };

    let GenericArgument::Type(body_type) = response_args.args.first()? else {
        return None;
    };

    Some(ResponseTypeInfo {
        body_type: body_type.clone(),
        body_type_str: body_type.to_token_stream().to_string(),
    })
}

/// Route decorator for async handler functions
///
/// Automatically registers the handler with the route registry.
/// Handlers receive `Request<T, C>` and return `Result<Response<U>, FlatbedError<D>>`.
///
/// # Attributes
/// - `method` (optional): HTTP method - GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS (default: POST)
/// - `version` (optional): API version for grouping routes (e.g., "v1", "v2")
/// - `tag` (optional): OpenAPI tag for grouping in docs
/// - `summary` (optional): Short description
/// - `operation_id` (optional): Unique operation identifier
/// - `deprecated` (optional): Mark as deprecated (default: false)
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::{route, Request, Response, FlatbedError};
///
/// // POST handler (default method)
/// #[route("/users", method = "POST", tag = "Users")]
/// async fn create_user(req: Request<CreateUserRequest>) -> Result<Response<UserResponse>, FlatbedError> {
///     Ok(Response::created(UserResponse { id: 1, name: req.body.name.clone() }))
/// }
///
/// // GET handler
/// #[route("/users/{id}", method = "GET", tag = "Users")]
/// async fn get_user(req: Request<EmptyRequest>) -> Result<Response<UserResponse>, FlatbedError> {
///     let id = req.param("id").unwrap();
///     Ok(Response::ok(UserResponse { id: id.parse().unwrap(), name: "John".into() }))
/// }
///
/// // PUT handler with context
/// #[route("/users/{id}", method = "PUT", tag = "Users")]
/// async fn update_user(req: Request<UpdateUserRequest, AppCtx>) -> Result<Response<UserResponse>, FlatbedError> {
///     let user = req.ctx.db.update_user(&req.body).await?;
///     Ok(Response::ok(user))
/// }
///
/// // DELETE handler
/// #[route("/users/{id}", method = "DELETE", tag = "Users")]
/// async fn delete_user(req: Request<EmptyRequest>) -> Result<Response<EmptyResponse>, FlatbedError> {
///     Ok(Response::no_content(EmptyResponse {}))
/// }
/// ```
#[proc_macro_attribute]
pub fn route(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as RouteAttrs);
    let input_fn = parse_macro_input!(item as ItemFn);
    let path = attrs.path;
    let method = attrs.method;
    let method_token = method.to_flatbed_token();

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_sig = &input_fn.sig;
    let fn_block = &input_fn.block;
    let fn_attrs = &input_fn.attrs;

    // Verify handler is async
    if fn_sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            fn_sig,
            "Route handler must be async. Use: async fn handle(...)",
        )
        .to_compile_error()
        .into();
    }

    // Extract input type from function signature
    let inputs = &fn_sig.inputs;
    let output = &fn_sig.output;

    // Parse request type (first parameter) - must be Request<T> or Request<T, C>
    let request_param_type = if let Some(syn::FnArg::Typed(pat_type)) = inputs.first() {
        &pat_type.ty
    } else {
        return syn::Error::new_spanned(
            fn_sig,
            "Route handler must have a Request<T> or Request<T, C> parameter",
        )
        .to_compile_error()
        .into();
    };

    let request_info = match parse_request_type(request_param_type) {
        Some(info) => info,
        None => {
            return syn::Error::new_spanned(
                request_param_type,
                "First parameter must be Request<T> or Request<T, C>",
            )
            .to_compile_error()
            .into();
        }
    };

    let body_type = &request_info.body_type;
    let has_context = request_info.has_context;
    let context_type = request_info
        .context_type
        .clone()
        .unwrap_or_else(|| syn::parse_quote!(()));

    // If context type is Arc<C>, extract the inner type for downcasting
    let ctx_inner_type = if has_context {
        match extract_arc_inner_type(&context_type) {
            Some(inner) => Some(inner),
            None => {
                return syn::Error::new_spanned(
                    &context_type,
                    "Route context type must be Arc<C> where C is your context type",
                )
                .to_compile_error()
                .into();
            }
        }
    } else {
        None
    };

    // Parse response type - must be Result<Response<T>, FlatbedError<D>>
    let response_info = match output {
        syn::ReturnType::Type(_, ty) => match parse_response_type(ty) {
            Some(info) => info,
            None => {
                return syn::Error::new_spanned(
                    ty,
                    "Return type must be Result<Response<T>, FlatbedError> or Result<Response<T>, FlatbedError<D>>",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                fn_sig,
                "Route handler must return Result<Response<T>, FlatbedError>",
            )
            .to_compile_error()
            .into();
        }
    };

    let response_body_type = &response_info.body_type;
    let response_body_type_str = &response_info.body_type_str;

    // Generate wrapper function name for hyper
    let wrapper_name = syn::Ident::new(&format!("__hyper_handler_{}", fn_name), fn_name.span());

    // Extract type names for metadata
    let body_type_str = body_type.to_token_stream().to_string();
    let body_type_name = extract_type_name(body_type);
    let response_body_type_name = {
        let full = response_body_type_str;
        let name = full.split("::").last().unwrap_or(full).trim();
        if let Some(idx) = name.find('<') {
            name[..idx].trim().to_string()
        } else {
            name.to_string()
        }
    };

    // Generate OpenAPI metadata tokens
    let version_token = match &attrs.version {
        Some(v) => quote! { Some(#v) },
        None => quote! { None },
    };
    let tag_token = match &attrs.tag {
        Some(t) => quote! { Some(#t) },
        None => quote! { None },
    };
    let summary_token = match &attrs.summary {
        Some(s) => quote! { Some(#s) },
        None => quote! { None },
    };
    let operation_id_token = match &attrs.operation_id {
        Some(o) => quote! { Some(#o) },
        None => quote! { None },
    };
    let deprecated_token = attrs.deprecated;

    // Schema names for OpenAPI
    let request_schema_name = body_type_name.clone();
    let response_schema_name = response_body_type_name;

    // Generate context extraction code based on whether handler uses context
    let ctx_extraction = if has_context {
        let inner = ctx_inner_type.as_ref().unwrap();
        quote! {
            let ctx = ctx_any.downcast::<#inner>()
                .map_err(|_| ::flatbed::Error::Custom(
                    format!("Route context type mismatch: expected {}", stringify!(#inner))
                ))?;
        }
    } else {
        quote! {}
    };

    let ctx_field = if has_context {
        quote! { ctx }
    } else {
        quote! { () }
    };

    let expanded = quote! {
        // Original async handler function
        #(#fn_attrs)*
        #fn_vis #fn_sig {
            #fn_block
        }

        // Hyper-compatible async handler wrapper
        //
        // This wrapper:
        // 1. Handles content-type negotiation (JSON / FlatBuffer)
        // 2. Deserializes the request body
        // 3. Builds the Request<T, C> struct with headers, params, etc.
        // 4. Calls the user handler
        // 5. Serializes the response
        // 6. Builds the ResponseParts for hyper
        #[allow(non_snake_case)]
        #[doc(hidden)]
        pub fn #wrapper_name(
            request_parts: ::flatbed::RequestParts,
            body: Vec<u8>,
            content_type: &str,
            ctx_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = Result<::flatbed::ResponseParts, ::flatbed::Error>> + Send>> {
            // Clone content_type into owned String to satisfy 'static lifetime
            let content_type = content_type.to_string();
            Box::pin(async move {
                use ::flatbed::ToFlatBuffer;

                let is_json = content_type.contains("application/json");
                // Support both "application/x-flatbuffers" and "application/x-flat-buffers" variants
                let is_flatbuffer = content_type.contains("application/x-flatbuffers")
                    || content_type.contains("application/x-flat-buffers");

                let response_content_type: &'static str = if is_json {
                    "application/json"
                } else {
                    "application/x-flatbuffers"
                };

                let request_id = request_parts.request_id.clone();

                // Deserialize request body
                let request_body: #body_type = if is_json {
                    match ::flatbed::serde_json::from_slice(&body) {
                        Ok(b) => b,
                        Err(e) => {
                            return Err(::flatbed::Error::DeserializationError(
                                format!("JSON deserialization error: {}", e)
                            ));
                        }
                    }
                } else if is_flatbuffer {
                    match #body_type::from_flatbuffer(&body) {
                        Ok(b) => b,
                        Err(e) => {
                            return Err(::flatbed::Error::DeserializationError(
                                format!("FlatBuffer deserialization error: {}", e)
                            ));
                        }
                    }
                } else {
                    // Empty body for methods that don't require one
                    match ::flatbed::serde_json::from_slice(b"{}") {
                        Ok(b) => b,
                        Err(e) => {
                            return Err(::flatbed::Error::DeserializationError(
                                format!("Empty body deserialization error: {}", e)
                            ));
                        }
                    }
                };

                // Extract application context (if handler uses Request<T, C>)
                #ctx_extraction

                // Build the Request<T, C>
                let request = ::flatbed::Request {
                    body: request_body,
                    ctx: #ctx_field,
                    headers: request_parts.headers,
                    method: request_parts.method,
                    path: request_parts.path,
                    path_params: request_parts.path_params,
                    query_params: request_parts.query_params,
                    request_id: request_parts.request_id,
                };

                // Call the actual handler
                let result = #fn_name(request).await;

                match result {
                    Ok(response) => {
                        // Serialize response body
                        let response_bytes = if is_json {
                            match ::flatbed::serde_json::to_vec(&response.body) {
                                Ok(b) => b,
                                Err(e) => {
                                    return Err(::flatbed::Error::SerializationError(
                                        format!("JSON serialization error: {}", e)
                                    ));
                                }
                            }
                        } else {
                            response.body.to_flatbuffer()
                        };

                        // Build response parts
                        let mut parts = ::flatbed::ResponseParts::with_status(
                            response_bytes,
                            response.status,
                            response_content_type,
                        );

                        // Add request ID header
                        parts = parts.with_request_id(&request_id);

                        // Copy response headers
                        for (key, value) in response.headers.iter() {
                            parts.headers.insert(key.clone(), value.clone());
                        }

                        Ok(parts)
                    }
                    Err(err) => {
                        // Build error response
                        let error_code = if err.code.is_empty() {
                            "ERROR"
                        } else {
                            &err.code
                        };

                        let mut headers = ::flatbed::HeaderMap::new();
                        if let Ok(val) = ::flatbed::HeaderValue::try_from(request_id.as_str()) {
                            headers.insert(
                                ::flatbed::HeaderName::from_static("x-request-id"),
                                val,
                            );
                        }

                        // Copy error headers
                        for (key, value) in err.headers.iter() {
                            headers.insert(key.clone(), value.clone());
                        }

                        let (body, content_type) = if is_flatbuffer {
                            // For FlatBuffer: code/message in headers, details in body
                            if let Ok(val) = ::flatbed::HeaderValue::try_from(error_code) {
                                headers.insert(
                                    ::flatbed::HeaderName::from_static("x-error-code"),
                                    val,
                                );
                            }
                            if let Ok(val) = ::flatbed::HeaderValue::try_from(err.message.as_str()) {
                                headers.insert(
                                    ::flatbed::HeaderName::from_static("x-error-message"),
                                    val,
                                );
                            }

                            let body = if let Some(ref details) = err.details {
                                details.to_flatbuffer()
                            } else {
                                Vec::new()
                            };

                            (body, "application/x-flatbuffers")
                        } else {
                            // For JSON: everything in body
                            #[derive(::flatbed::serde::Serialize)]
                            struct ErrorBody<'a, D: ::flatbed::serde::Serialize> {
                                code: &'a str,
                                message: &'a str,
                                #[serde(skip_serializing_if = "Option::is_none")]
                                details: &'a Option<D>,
                            }

                            let error_body = ErrorBody {
                                code: error_code,
                                message: &err.message,
                                details: &err.details,
                            };

                            let body = ::flatbed::serde_json::to_vec(&error_body).unwrap_or_default();
                            (body, "application/json")
                        };

                        Ok(::flatbed::ResponseParts {
                            body,
                            status: err.status,
                            headers,
                            content_type,
                        })
                    }
                }
            })
        }

        // Register route info for discovery
        ::flatbed::inventory::submit! {
            #[allow(non_upper_case_globals)]
            #[doc(hidden)]
            ::flatbed::RouteInfo {
                path: #path,
                method: #method_token,
                request_type: #body_type_str,
                response_type: #response_body_type_str,
                handler: |_, _| panic!("Legacy handler called - use async handler instead"),
                async_handler: #wrapper_name,
                openapi: ::flatbed::OpenApiRouteInfo {
                    version: #version_token,
                    tag: #tag_token,
                    summary: #summary_token,
                    operation_id: #operation_id_token,
                    deprecated: #deprecated_token,
                    request_schema: Some(::flatbed::SchemaInfo {
                        name: #request_schema_name,
                        fields: <#body_type as ::flatbed::ToFlatBuffer>::SCHEMA_FIELDS,
                    }),
                    response_schema: Some(::flatbed::SchemaInfo {
                        name: #response_schema_name,
                        fields: <#response_body_type as ::flatbed::ToFlatBuffer>::SCHEMA_FIELDS,
                    }),
                },
            }
        }
    };

    TokenStream::from(expanded)
}

/// Entry point macro for flatbed applications
///
/// Sets up the tokio runtime, validates routes, builds the router,
/// and starts the HTTP server.
///
/// # Attributes
/// - `bind` (required): Socket address to bind to (e.g., "0.0.0.0:8080")
/// - `context` (optional): Application context type
/// - `init` (optional): Async function to initialize context
///
/// # Example
///
/// ```rust,ignore
/// use flatbed::main;
///
/// #[flatbed::main(bind = "0.0.0.0:8080")]
/// async fn main() {
///     println!("Server started");
/// }
///
/// // With context:
/// #[flatbed::main(
///     bind = "0.0.0.0:8080",
///     context = AppContext,
///     init = init_context,
/// )]
/// async fn main() { }
/// ```
#[proc_macro_attribute]
pub fn main(attr: TokenStream, item: TokenStream) -> TokenStream {
    main_macro::main_impl(attr, item)
}
