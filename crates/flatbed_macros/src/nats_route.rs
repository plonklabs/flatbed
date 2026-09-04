use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Ident, ItemFn, LitStr, Token,
};

use crate::{extract_arc_inner_type, parse_request_type, parse_response_type};

/// Parsed attributes for the `#[nats_route]` macro.
struct NatsRouteAttrs {
    subject: LitStr,
    queue: Option<LitStr>,
}

impl Parse for NatsRouteAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let subject: LitStr = input.parse()?;
        let mut queue = None;

        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }

            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match key.to_string().as_str() {
                "queue" => queue = Some(input.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown nats_route key `{other}`; expected `queue`"),
                    ))
                }
            }
        }

        Ok(Self { subject, queue })
    }
}

/// A subject pattern split into the subject actually subscribed to and the
/// `{token}` captures, each paired with the subject-token index it reads.
#[derive(Debug)]
struct SubjectPattern {
    wire: String,
    params: Vec<(String, usize)>,
}

/// Translate a declared subject pattern into its wire form.
///
/// `{token}` segments become NATS single-token wildcards and are recorded as
/// named captures. Raw `*` and `>` wildcards are rejected: they would capture
/// nothing under a name, and `>` spans an unbounded number of tokens, which
/// no fixed capture index can address.
fn parse_subject(pattern: &str) -> Result<SubjectPattern, String> {
    if pattern.is_empty() {
        return Err("subject must not be empty".to_string());
    }

    let mut wire = Vec::new();
    let mut params: Vec<(String, usize)> = Vec::new();

    for (index, token) in pattern.split('.').enumerate() {
        if token.is_empty() {
            return Err(format!("subject '{pattern}' has an empty token"));
        }
        if token == "*" || token == ">" {
            return Err(format!(
                "subject '{pattern}' uses a raw '{token}' wildcard; \
                 name the segment as '{{token}}' instead"
            ));
        }

        let Some(name) = token.strip_prefix('{').and_then(|t| t.strip_suffix('}')) else {
            if token.contains(['{', '}', '*', '>']) {
                return Err(format!(
                    "subject '{pattern}' is neither a literal nor a \
                     whole '{{token}}' segment at '{token}'"
                ));
            }
            // The broker rejects a subject carrying whitespace at subscribe
            // time.
            if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!(
                    "subject '{pattern}' token '{token}' contains whitespace"
                ));
            }
            wire.push(token.to_string());
            continue;
        };

        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "subject '{pattern}' token '{token}' must name a parameter using \
                 letters, digits, or underscores"
            ));
        }
        if params.iter().any(|(existing, _)| existing == name) {
            return Err(format!(
                "subject '{pattern}' declares the parameter '{name}' twice"
            ));
        }

        params.push((name.to_string(), index));
        wire.push("*".to_string());
    }

    Ok(SubjectPattern {
        wire: wire.join("."),
        params,
    })
}

/// Expand `#[nats_route]` into the handler, its dispatch wrapper, and the
/// `inventory` entries that make the responder discoverable and spawnable.
pub fn nats_route_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as NatsRouteAttrs);
    let input_fn = parse_macro_input!(item as ItemFn);

    let subject = attrs.subject;
    let pattern = match parse_subject(&subject.value()) {
        Ok(parsed) => parsed,
        Err(message) => {
            return syn::Error::new(subject.span(), message)
                .to_compile_error()
                .into()
        }
    };

    if let Some(queue) = attrs.queue.as_ref().filter(|q| q.value().is_empty()) {
        return syn::Error::new(
            queue.span(),
            "queue group must not be empty; drop the `queue` key to subscribe without one",
        )
        .to_compile_error()
        .into();
    }

    let fn_name = &input_fn.sig.ident;
    let fn_vis = &input_fn.vis;
    let fn_sig = &input_fn.sig;
    let fn_block = &input_fn.block;
    let fn_attrs = &input_fn.attrs;

    if fn_sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            fn_sig,
            "nats_route handler must be async. Use: async fn handle(...)",
        )
        .to_compile_error()
        .into();
    }

    // The wrapper calls the handler with no turbofish and exactly one
    // argument, so any other signature fails to compile with its error
    // spanned at generated code rather than at the declaration.
    if !fn_sig.generics.params.is_empty() || fn_sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(&fn_sig.generics, "nats_route handler must not be generic")
            .to_compile_error()
            .into();
    }

    if fn_sig.inputs.len() != 1 {
        return syn::Error::new_spanned(
            &fn_sig.inputs,
            "nats_route handler takes exactly one Request<T, Arc<C>> parameter",
        )
        .to_compile_error()
        .into();
    }

    let Some(syn::FnArg::Typed(first_param)) = fn_sig.inputs.first() else {
        return syn::Error::new_spanned(
            fn_sig,
            "nats_route handler must have a Request<T, Arc<C>> parameter",
        )
        .to_compile_error()
        .into();
    };
    let request_param_type = &first_param.ty;

    let Some(request_info) = parse_request_type(request_param_type) else {
        return syn::Error::new_spanned(
            request_param_type,
            "First parameter must be Request<T, Arc<C>>",
        )
        .to_compile_error()
        .into();
    };

    // Without `Arc<C>` there is no connection to subscribe on, so a subject
    // responder has no contextless form.
    let has_context = request_info.has_context;
    let Some(context_type) = request_info.context_type.filter(|_| has_context) else {
        return syn::Error::new_spanned(
            request_param_type,
            "nats_route handler must take Request<T, Arc<C>> where C implements \
             flatbed::HasNatsClient — the responder subscribes through the context's client",
        )
        .to_compile_error()
        .into();
    };

    let Some(ctx_inner_type) = extract_arc_inner_type(&context_type) else {
        return syn::Error::new_spanned(
            &context_type,
            "nats_route context type must be Arc<C> where C is your context type",
        )
        .to_compile_error()
        .into();
    };

    let response_info = match &fn_sig.output {
        syn::ReturnType::Type(_, ty) => match parse_response_type(ty) {
            Some(info) => info,
            None => {
                return syn::Error::new_spanned(
                    ty,
                    "Return type must be Result<Response<T>, FlatbedError> or \
                     Result<Response<T>, FlatbedError<D>>",
                )
                .to_compile_error()
                .into()
            }
        },
        _ => {
            return syn::Error::new_spanned(
                fn_sig,
                "nats_route handler must return Result<Response<T>, FlatbedError>",
            )
            .to_compile_error()
            .into()
        }
    };

    let body_type = &request_info.body_type;
    let body_type_str = body_type.to_token_stream().to_string();
    let response_type_str = &response_info.body_type_str;

    let wrapper_name = syn::Ident::new(&format!("__nats_handler_{fn_name}"), fn_name.span());
    // The handler's name is used verbatim: uppercasing it collides for two
    // handlers in one module whose names differ only in case.
    let route_const = syn::Ident::new(&format!("__NATS_ROUTE_{fn_name}"), fn_name.span());

    let wire_subject = pattern.wire;
    let param_names = pattern.params.iter().map(|(name, _)| name);
    let param_indexes = pattern.params.iter().map(|(_, index)| index);
    let queue_token = match &attrs.queue {
        Some(queue) => quote! { Some(#queue) },
        None => quote! { None },
    };

    let expanded = quote! {
        #(#fn_attrs)*
        #fn_vis #fn_sig {
            #fn_block
        }

        // Every path returns a reply: a decode failure, a context mismatch,
        // and a handler error all become error replies.
        #[allow(non_snake_case)]
        #[doc(hidden)]
        pub fn #wrapper_name(
            parts: ::flatbed::nats_route::NatsRequestParts,
            payload: Vec<u8>,
            ctx_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
        ) -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ::flatbed::nats_route::NatsReply> + Send>> {
            Box::pin(async move {
                let encoding = parts.encoding;
                let request_id = parts.request_id.clone();

                let body: #body_type = match encoding {
                    ::flatbed::nats_route::NatsEncoding::Json => {
                        match ::flatbed::serde_json::from_slice(&payload) {
                            Ok(body) => body,
                            Err(e) => {
                                return ::flatbed::nats_route::reply_err(
                                    encoding,
                                    &request_id,
                                    &::flatbed::FlatbedRouteError::bad_request(
                                        format!("JSON deserialization error: {}", e)
                                    ).code("DESERIALIZATION_ERROR"),
                                );
                            }
                        }
                    }
                    ::flatbed::nats_route::NatsEncoding::FlatBuffer => {
                        match #body_type::from_flatbuffer(&payload) {
                            Ok(body) => body,
                            Err(e) => {
                                return ::flatbed::nats_route::reply_err(
                                    encoding,
                                    &request_id,
                                    &::flatbed::FlatbedRouteError::bad_request(
                                        format!("FlatBuffer deserialization error: {}", e)
                                    ).code("DESERIALIZATION_ERROR"),
                                );
                            }
                        }
                    }
                };

                let ctx = match ctx_any.downcast::<#ctx_inner_type>() {
                    Ok(ctx) => ctx,
                    Err(_) => {
                        return ::flatbed::nats_route::reply_err(
                            encoding,
                            &request_id,
                            &::flatbed::FlatbedRouteError::internal(format!(
                                "nats_route context type mismatch: expected {}",
                                stringify!(#ctx_inner_type)
                            )).code("CONTEXT_TYPE_MISMATCH"),
                        );
                    }
                };

                let request = ::flatbed::Request {
                    body,
                    ctx,
                    headers: parts.headers,
                    method: ::flatbed::Method::POST,
                    path: parts.subject,
                    path_params: parts.params,
                    query_params: ::std::collections::HashMap::new(),
                    request_id: parts.request_id,
                };

                match #fn_name(request).await {
                    Ok(response) => ::flatbed::nats_route::reply_ok(encoding, &request_id, response),
                    Err(err) => ::flatbed::nats_route::reply_err(encoding, &request_id, &err),
                }
            })
        }

        #[allow(non_upper_case_globals)]
        #[doc(hidden)]
        pub const #route_const: ::flatbed::nats_route::NatsRouteInfo =
            ::flatbed::nats_route::NatsRouteInfo {
                subject: #subject,
                wire_subject: #wire_subject,
                params: &[#((#param_names, #param_indexes)),*],
                queue: #queue_token,
                request_type: #body_type_str,
                response_type: #response_type_str,
                handler: #wrapper_name,
            };

        ::flatbed::inventory::submit! { #route_const }

        // The responder runs as a worker so a subscription that fails or ends
        // takes the process down the same way any other worker failure does.
        ::flatbed::inventory::submit! {
            ::flatbed::WorkerInfo {
                name: concat!("nats_route:", #subject),
                description: None,
                worker: {
                    fn __worker(
                        ctx: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
                    ) -> ::std::pin::Pin<
                        Box<dyn ::std::future::Future<Output = ::std::result::Result<(), ::flatbed::FlatbedWorkerError>> + Send>,
                    > {
                        Box::pin(::flatbed::nats_route::run_nats_route::<#ctx_inner_type>(
                            ctx,
                            #route_const,
                        ))
                    }
                    __worker
                },
            }
        }
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::parse_subject;

    #[test]
    fn a_literal_subject_subscribes_verbatim_and_captures_nothing() {
        let parsed = parse_subject("plonk.ground.report.worldstate").unwrap();
        assert_eq!(parsed.wire, "plonk.ground.report.worldstate");
        assert!(parsed.params.is_empty());
    }

    #[test]
    fn token_segments_become_wildcards_paired_with_their_index() {
        let parsed = parse_subject("plonk.satellite.{id}.call.{verb}").unwrap();
        assert_eq!(parsed.wire, "plonk.satellite.*.call.*");
        assert_eq!(
            parsed.params,
            vec![("id".to_string(), 2), ("verb".to_string(), 4)]
        );
    }

    #[test]
    fn a_leading_token_segment_is_captured_at_index_zero() {
        let parsed = parse_subject("{tenant}.events").unwrap();
        assert_eq!(parsed.wire, "*.events");
        assert_eq!(parsed.params, vec![("tenant".to_string(), 0)]);
    }

    #[test]
    fn raw_nats_wildcards_are_rejected_in_favour_of_named_tokens() {
        assert!(parse_subject("plonk.*.status")
            .unwrap_err()
            .contains("raw '*' wildcard"));
        assert!(parse_subject("plonk.>")
            .unwrap_err()
            .contains("raw '>' wildcard"));
    }

    #[test]
    fn a_token_must_span_a_whole_segment() {
        assert!(parse_subject("plonk.sat-{id}.status")
            .unwrap_err()
            .contains("neither a literal nor a whole"));
    }

    #[test]
    fn malformed_subjects_are_rejected() {
        assert!(parse_subject("").unwrap_err().contains("must not be empty"));
        assert!(parse_subject("plonk..status")
            .unwrap_err()
            .contains("empty token"));
        assert!(parse_subject("plonk.{}.status")
            .unwrap_err()
            .contains("must name a parameter"));
        assert!(parse_subject("plonk.{a.b}.status")
            .unwrap_err()
            .contains("neither a literal nor a whole"));
    }

    #[test]
    fn a_literal_token_with_whitespace_is_rejected() {
        assert!(parse_subject("plonk.hello world")
            .unwrap_err()
            .contains("contains whitespace"));
    }

    #[test]
    fn a_repeated_parameter_name_is_rejected() {
        assert!(parse_subject("plonk.{id}.call.{id}")
            .unwrap_err()
            .contains("declares the parameter 'id' twice"));
    }
}
