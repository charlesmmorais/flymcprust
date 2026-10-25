//! Procedural macros for SDKMCPRust. Use them through the `sdkmcprust` crate,
//! whose documentation describes them in detail.
#![forbid(unsafe_code)]

mod docs;
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
