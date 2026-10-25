//! Procedural macros for SDKMCPRust. Use them through the `sdkmcprust` crate,
//! whose documentation describes them in detail.
#![forbid(unsafe_code)]

mod docs;
mod params;
mod prompt;
mod resource;
mod tool;

use proc_macro::TokenStream;

/// Turns a free function (sync or `async`) into an MCP tool.
///
/// ```ignore
/// /// Adds two numbers.
/// ///
/// /// # Arguments
/// /// * `a` - First operand.
/// /// * `b` - Second operand.
/// #[tool(read_only, idempotent)]
/// async fn add(a: f64, b: f64) -> f64 {
///     a + b
/// }
///
/// Server::builder("calc", "1.0.0").tool::<add>().build()?;
/// ```
///
/// - The doc comment becomes the tool description; the `# Arguments` list
///   becomes the parameter descriptions.
/// - Parameters become the JSON Schema of `inputSchema`. `Ctx` and
///   `State<T>` parameters are injected and excluded from the schema.
///   `#[serde(..)]` and `#[schemars(..)]` attributes on parameters are
///   forwarded to the generated schema.
/// - Returning `Json<T>` (or `Result<Json<T>, E>`) generates `outputSchema`.
/// - Arguments: `name = "..."`, `title = "..."`, `description = "..."`, and
///   the annotation flags `read_only`, `destructive`, `idempotent`,
///   `open_world` (each optionally `= true/false`).
///
/// The function stays callable as before; the macro adds a marker type with
/// the same name (in the type namespace) that implements `Tool`.
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns a free function (sync or `async`) into an MCP resource or resource
/// template.
///
/// ```ignore
/// /// The application configuration.
/// #[resource("config://app", mime_type = "application/json")]
/// fn app_config() -> Json<Config> { .. }
///
/// /// One user record.
/// #[resource(template = "users://{id}/profile", mime_type = "application/json")]
/// async fn user(#[complete(user_ids)] id: u64, db: State<Db>) -> Result<String, ToolError> { .. }
/// ```
///
/// - A string literal is a fixed URI (listed by `resources/list`);
///   `template = ".."` is an RFC 6570 URI template (listed by
///   `resources/templates/list`).
/// - Template variables become parameters of the same name, parsed with
///   `FromStr`; a value that does not parse means the resource does not
///   exist. Query variables (`{?q}`) must be `Option<T>`.
/// - `Ctx` and `State<T>` parameters are injected.
/// - `#[complete(path)]` or `#[complete(values = [..])]` on a variable adds
///   argument completion.
/// - Arguments: `name`, `title`, `description`, `mime_type`, `ttl_ms`
///   (default 0) and `cache = "public" | "private"` (default `"private"`).
/// - The return type implements `IntoResourceContents`: `String` (text),
///   `Vec<u8>` (binary), `Json<T>`, the specification's contents types, or a
///   `Result` of these.
#[proc_macro_attribute]
pub fn resource(attr: TokenStream, item: TokenStream) -> TokenStream {
    resource::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns a free function (sync or `async`) into an MCP prompt.
///
/// ```ignore
/// /// Reviews a piece of code.
/// ///
/// /// # Arguments
/// /// * `code` - The code to review.
/// /// * `language` - Its programming language.
/// #[prompt]
/// fn code_review(code: String, #[complete(values = ["rust", "python"])] language: Option<String>) -> String {
///     format!("Review this {} code: {code}", language.as_deref().unwrap_or("source"))
/// }
/// ```
///
/// - Parameters are prompt arguments, parsed with `FromStr`; `Option<T>`
///   arguments are optional. `Ctx` and `State<T>` are injected.
/// - The doc comment becomes the description; the `# Arguments` list (or a
///   doc comment on the parameter) describes each argument.
/// - `#[complete(..)]` on an argument adds argument completion.
/// - Arguments: `name`, `title`, `description`.
/// - The return type implements `IntoPromptResult`: `String` (one user
///   message), `PromptMessage`, `Vec<PromptMessage>`, `GetPromptResult`, or a
///   `Result` of these.
#[proc_macro_attribute]
pub fn prompt(attr: TokenStream, item: TokenStream) -> TokenStream {
    prompt::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
