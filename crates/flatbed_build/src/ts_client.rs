//! Generate `client.ts`: a zero-dependency `fetch` client that calls each
//! FlatBuffer operation of a flatbed service.
//!
//! Each method encodes the typed request with the generated codec, sends it as
//! `application/x-flatbuffers` using the operation's HTTP method, and decodes
//! the response the same way. Path parameters (`/users/{id}`) become leading
//! string arguments.

use std::collections::BTreeMap;

use crate::fb_plugin::FbOperation;

const CONTENT_TYPE: &str = "application/x-flatbuffers";

/// Method names that would collide with a member `FlatbedClient` always emits —
/// the `request` method, the `options` constructor-parameter property, and the
/// `constructor` itself. A derived name matching one compiles to a
/// duplicate-identifier error.
const RESERVED_METHOD_NAMES: &[&str] = &["constructor", "options", "request"];

/// Reserved words TypeScript rejects as a binding name inside a generated
/// `async` method — an argument named one of these fails to parse. Verified
/// against `tsc --strict`; contextual keywords (`type`, `interface`, `get`,
/// `let`, `catch`, …) are legal argument names and deliberately absent.
const TS_KEYWORDS: &[&str] = &[
    "await",
    "break",
    "case",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
];

/// A path parameter rendered as a valid TypeScript identifier for the generated
/// method. The name is a pure local — the method argument and the URL-template
/// interpolation, never part of the wire contract — so it can be adjusted
/// freely: a reserved word gets a trailing `_`, and a name that isn't a valid
/// identifier (empty or digit-leading like `2fa`) gets a leading `_`.
fn param_ident(param: &str) -> String {
    let base = camel_case(param);
    if TS_KEYWORDS.contains(&base.as_str()) {
        format!("{base}_")
    } else if is_valid_ts_identifier(&base) {
        base
    } else {
        format!("_{base}")
    }
}

/// Whether `name` is a valid TypeScript identifier: non-empty, starting with a
/// letter, `_`, or `$`, and otherwise made of letters, digits, `_`, or `$`. A
/// derived name that isn't (empty, or digit-leading like `1start`) would emit a
/// method declaration that fails to parse.
fn is_valid_ts_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// Error if any operation derives a client method name that won't compile: an
/// invalid TypeScript identifier, a name reserved by `FlatbedClient`, or a
/// duplicate of another operation's name.
pub(crate) fn check_unique_method_names(ops: &[FbOperation]) -> Result<(), String> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for op in ops {
        let name = method_name(op);
        let sig = format!("{} {}", op.method, op.path);
        if !is_valid_ts_identifier(&name) {
            return Err(format!(
                "operation `{sig}` derives the client method name `{name}`, which is not a valid \
                 TypeScript identifier — set an explicit operationId"
            ));
        }
        if RESERVED_METHOD_NAMES.contains(&name.as_str()) {
            return Err(format!(
                "operation `{sig}` derives the client method name `{name}`, which collides with a \
                 member `FlatbedClient` already defines — set a different operationId"
            ));
        }
        if let Some(prev) = seen.insert(name.clone(), sig.clone()) {
            return Err(format!(
                "two operations derive the same client method `{name}`: `{prev}` and `{sig}` — \
                 give one an explicit operationId to disambiguate"
            ));
        }
    }
    Ok(())
}

/// The request type the generated method actually encodes. GET/HEAD never carry
/// a body (browser `fetch` rejects it), so their request type is dropped — both
/// the emitted method and the imports it drives must agree on this, or the
/// generated file carries an unused import that trips `noUnusedLocals`.
fn effective_request_type(op: &FbOperation) -> Option<&str> {
    match op.method.as_str() {
        "GET" | "HEAD" => None,
        _ => op.request_type.as_deref(),
    }
}

/// Generate `client.ts` for the FlatBuffer operations discovered in the spec.
pub(crate) fn generate(ops: &[FbOperation]) -> String {
    let mut imports_types: Vec<&str> = Vec::new();
    for op in ops {
        imports_types.extend(effective_request_type(op));
        imports_types.extend(op.response_type.as_deref());
    }
    imports_types.sort_unstable();
    imports_types.dedup();

    let mut out = String::from("// Auto-generated by flatbed — do not edit.\n\n");
    // The codec import is only referenced by operations that encode a request or
    // decode a response; emitting it otherwise trips `noUnusedLocals`.
    if ops
        .iter()
        .any(|op| effective_request_type(op).is_some() || op.response_type.is_some())
    {
        out.push_str("import * as codec from \"./codec.js\";\n");
    }
    if !imports_types.is_empty() {
        out.push_str(&format!(
            "import type {{ {} }} from \"./types.js\";\n",
            imports_types.join(", ")
        ));
    }
    out.push('\n');
    out.push_str(&format!("const CONTENT_TYPE = \"{CONTENT_TYPE}\";\n\n"));
    out.push_str(PREAMBLE);

    out.push_str("export class FlatbedClient {\n");
    out.push_str("  constructor(private readonly options: ClientOptions) {}\n\n");
    for op in ops {
        out.push_str(&method(op));
    }
    out.push_str(REQUEST_METHOD);
    out.push_str("}\n");
    out
}

fn method(op: &FbOperation) -> String {
    let name = method_name(op);
    let params = path_params(&op.path);

    // Path-param names can be non-identifiers (`item-id`) or reserved words
    // (`type`), so the argument uses a sanitized identifier form.
    let mut args: Vec<String> = params
        .iter()
        .map(|p| format!("{}: string", param_ident(p)))
        .collect();
    let ret_type = response_type(op);
    let body_expr = match effective_request_type(op) {
        Some(req) => {
            args.push(format!("body: {req}"));
            format!("codec.encode{req}Root(body)")
        }
        None => "new Uint8Array()".to_string(),
    };
    let decode = match &op.response_type {
        Some(res) => format!("codec.decode{res}Root"),
        None => "(bytes: Uint8Array) => bytes".to_string(),
    };

    let path_expr = if params.is_empty() {
        format!("\"{}\"", op.path)
    } else {
        let mut tmpl = op.path.clone();
        for p in &params {
            tmpl = tmpl.replace(&format!("{{{p}}}"), &format!("${{{}}}", param_ident(p)));
        }
        format!("`{tmpl}`")
    };

    format!(
        "  async {name}({args}): Promise<{ret_type}> {{\n\
         \x20   return this.request(\"{method}\", {path_expr}, {body_expr}, {decode});\n\
         \x20 }}\n\n",
        args = args.join(", "),
        method = op.method,
    )
}

/// `operationId` when present, else `<method><PascalPathSegments>` — e.g.
/// `GET /users/{id}` → `getUsersById`.
fn method_name(op: &FbOperation) -> String {
    if let Some(id) = &op.operation_id {
        return camel_case(id);
    }
    let mut name = op.method.to_lowercase();
    for seg in op.path.split('/').filter(|s| !s.is_empty()) {
        let word = match seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(param) => format!("By{}", pascal_case(param)),
            None => pascal_case(seg),
        };
        name.push_str(&word);
    }
    name
}

fn response_type(op: &FbOperation) -> String {
    match &op.response_type {
        Some(res) => res.clone(),
        None => "Uint8Array".to_string(),
    }
}

/// The `{param}` names in a path, in order.
fn path_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| {
            seg.strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .map(str::to_string)
        })
        .collect()
}

/// Turn an `operationId` into a camelCase method name: `GetUser`, `get_user`
/// and `get-user` all become `getUser`.
fn camel_case(s: &str) -> String {
    let mut out = String::new();
    let mut first = true;
    let mut capitalize_next = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            capitalize_next = !first;
        } else if first {
            out.extend(c.to_lowercase());
            first = false;
        } else if capitalize_next {
            out.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn pascal(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => s.to_string(),
    }
}

/// PascalCase form of a path-segment fragment for a method name: separators are
/// dropped with each following word capitalized, and the first character is
/// uppercased (`user-id` → `UserId`).
fn pascal_case(s: &str) -> String {
    pascal(&camel_case(s))
}

const PREAMBLE: &str = "\
export interface ClientOptions {
  baseUrl: string;
  fetch?: typeof globalThis.fetch;
}

export class FlatbedError extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
    this.name = \"FlatbedError\";
  }
}

";

const REQUEST_METHOD: &str = "  private async request<T>(
    method: string,
    path: string,
    body: Uint8Array,
    decode: (bytes: Uint8Array) => T,
  ): Promise<T> {
    const fetchImpl = this.options.fetch ?? globalThis.fetch;
    const init: RequestInit = { method, headers: { accept: CONTENT_TYPE } };
    // Browser `fetch` rejects a body on GET/HEAD, so never attach one there.
    if (method !== \"GET\" && method !== \"HEAD\" && body.length > 0) {
      (init.headers as Record<string, string>)[\"content-type\"] = CONTENT_TYPE;
      init.body = body as BodyInit;
    }
    const base = this.options.baseUrl.replace(/\\/+$/, \"\");
    const res = await fetchImpl(base + path, init);
    if (!res.ok) {
      throw new FlatbedError(res.status, `${method} ${path} failed: ${res.status}`);
    }
    return decode(new Uint8Array(await res.arrayBuffer()));
  }
";

#[cfg(test)]
mod tests {
    use super::*;

    fn op(method: &str, path: &str, req: Option<&str>, res: Option<&str>) -> FbOperation {
        FbOperation {
            path: path.to_string(),
            method: method.to_string(),
            operation_id: None,
            request_type: req.map(str::to_string),
            response_type: res.map(str::to_string),
        }
    }

    #[test]
    fn method_encodes_request_and_decodes_response() {
        let client = generate(&[op(
            "POST",
            "/echo",
            Some("EchoRequest"),
            Some("EchoResponse"),
        )]);
        assert!(client.contains("import type { EchoRequest, EchoResponse } from \"./types.js\";"));
        assert!(client.contains("async postEcho(body: EchoRequest): Promise<EchoResponse> {"));
        assert!(client.contains(
            "return this.request(\"POST\", \"/echo\", codec.encodeEchoRequestRoot(body), codec.decodeEchoResponseRoot);"
        ));
    }

    #[test]
    fn path_params_become_leading_string_args() {
        let client = generate(&[op("POST", "/users/{id}", Some("Patch"), Some("User"))]);
        assert!(client.contains("async postUsersById(id: string, body: Patch): Promise<User> {"));
        assert!(client.contains("return this.request(\"POST\", `/users/${id}`,"));
    }

    #[test]
    fn get_with_request_type_omits_body_argument() {
        let client = generate(&[op("GET", "/users/{id}", Some("Empty"), Some("User"))]);
        assert!(client.contains("async getUsersById(id: string): Promise<User> {"));
        assert!(client.contains("return this.request(\"GET\", `/users/${id}`, new Uint8Array(),"));
    }

    #[test]
    fn get_only_request_type_drives_no_imports() {
        let client = generate(&[op("GET", "/ping", Some("Ping"), None)]);
        assert!(!client.contains("import * as codec"));
        assert!(!client.contains("import type"));
        assert!(client.contains("async getPing(): Promise<Uint8Array> {"));
    }

    #[test]
    fn hyphenated_path_param_becomes_valid_identifier() {
        let client = generate(&[op("GET", "/items/{item-id}", None, Some("Item"))]);
        assert!(client.contains("async getItemsByItemId(itemId: string): Promise<Item> {"));
        assert!(client.contains("`/items/${itemId}`"));
    }

    #[test]
    fn reserved_word_path_param_is_sanitized() {
        // `class` is a reserved argument name; the local gets a trailing `_`, but
        // the derived method name keeps its `By{Segment}` form.
        let client = generate(&[op("GET", "/items/{class}", None, Some("Item"))]);
        assert!(client.contains("async getItemsByClass(class_: string): Promise<Item> {"));
        assert!(client.contains("`/items/${class_}`"));
    }

    #[test]
    fn digit_leading_path_param_is_sanitized() {
        let client = generate(&[op("GET", "/mfa/{2fa}", None, Some("Mfa"))]);
        assert!(client.contains("async getMfaBy2fa(_2fa: string): Promise<Mfa> {"));
        assert!(client.contains("`/mfa/${_2fa}`"));
    }

    #[test]
    fn non_get_with_response_only_emits_codec_and_no_body() {
        let client = generate(&[op("DELETE", "/users/{id}", None, Some("User"))]);
        assert!(client.contains("import * as codec"));
        assert!(client.contains("async deleteUsersById(id: string): Promise<User> {"));
        assert!(client.contains("new Uint8Array(), codec.decodeUserRoot"));
    }

    #[test]
    fn operation_id_wins_over_derived_name() {
        let mut o = op("POST", "/echo", Some("EchoRequest"), Some("EchoResponse"));
        o.operation_id = Some("EchoMessage".to_string());
        let client = generate(&[o]);
        assert!(client.contains("async echoMessage("));
    }

    #[test]
    fn missing_response_type_returns_raw_bytes() {
        let client = generate(&[op("POST", "/raw", Some("Req"), None)]);
        assert!(client.contains("Promise<Uint8Array>"));
        assert!(client.contains("(bytes: Uint8Array) => bytes"));
    }

    #[test]
    fn get_never_attaches_a_body() {
        let client = generate(&[op("GET", "/health", None, Some("Health"))]);
        assert!(client.contains("async getHealth(): Promise<Health> {"));
        assert!(
            client.contains("if (method !== \"GET\" && method !== \"HEAD\" && body.length > 0) {")
        );
    }

    #[test]
    fn base_url_trailing_slash_is_trimmed() {
        let client = generate(&[op("POST", "/echo", Some("Req"), Some("Res"))]);
        assert!(client.contains("this.options.baseUrl.replace(/\\/+$/, \"\")"));
    }

    #[test]
    fn codec_import_omitted_when_no_operation_is_typed() {
        let client = generate(&[op("GET", "/ping", None, None)]);
        assert!(!client.contains("import * as codec"));
        assert!(client.contains("async getPing(): Promise<Uint8Array> {"));
    }

    #[test]
    fn colliding_method_names_are_rejected() {
        let mut a = op("GET", "/users", None, Some("User"));
        a.operation_id = Some("fetch".to_string());
        let mut b = op("POST", "/people", Some("Person"), Some("Person"));
        b.operation_id = Some("fetch".to_string());
        let err = check_unique_method_names(&[a.clone(), b]).expect_err("duplicate must fail");
        assert!(err.contains("`fetch`"), "message: {err}");
        assert!(check_unique_method_names(&[a]).is_ok());
    }

    #[test]
    fn path_derived_names_that_collide_are_rejected() {
        // `foo-bar` and `fooBar` both collapse to `getFooBar` with no operationId.
        let ops = [
            op("GET", "/foo-bar", None, Some("Foo")),
            op("GET", "/fooBar", None, Some("Foo")),
        ];
        let err = check_unique_method_names(&ops).expect_err("collision must fail");
        assert!(err.contains("`getFooBar`"), "message: {err}");
    }

    #[test]
    fn reserved_method_names_are_rejected() {
        for reserved in RESERVED_METHOD_NAMES {
            let mut o = op("POST", "/x", Some("Req"), Some("Res"));
            o.operation_id = Some((*reserved).to_string());
            let err = check_unique_method_names(&[o])
                .expect_err(&format!("reserved name `{reserved}` must fail"));
            assert!(err.contains("collides"), "message: {err}");
        }
    }

    #[test]
    fn digit_leading_method_name_is_rejected() {
        let mut o = op("POST", "/x", Some("Req"), Some("Res"));
        o.operation_id = Some("1start".to_string());
        let err = check_unique_method_names(&[o]).expect_err("invalid identifier must fail");
        assert!(
            err.contains("valid TypeScript identifier"),
            "message: {err}"
        );
    }

    #[test]
    fn snake_case_operation_id_becomes_camel() {
        let mut o = op("POST", "/x", Some("Req"), Some("Res"));
        o.operation_id = Some("create_user".to_string());
        let client = generate(&[o]);
        assert!(client.contains("async createUser("));
    }
}
