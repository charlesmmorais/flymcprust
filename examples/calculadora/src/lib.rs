//! A calculator exposed as an MCP server.
//!
//! The tools live in this library so the binary and the integration tests
//! share them. Run the server with `cargo run -p calculadora` (stdio) or
//! `cargo run -p calculadora -- --http 127.0.0.1:8000` (Streamable HTTP).
#![forbid(unsafe_code)]

use schemars::JsonSchema;
use sdkmcprust::prelude::*;
use sdkmcprust::{BuildError, ServerBuilder};
use serde::Serialize;

/// Rejects results JSON cannot represent (infinities, NaN).
fn finite(value: f64) -> Result<f64, ToolError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ToolError::user(
            "the result is not a finite number (overflow or invalid operation)",
        ))
    }
}

/// Adds two numbers.
///
/// # Arguments
/// * `a` - The first addend.
/// * `b` - The second addend.
#[tool(read_only, idempotent, open_world = false)]
pub async fn add(a: f64, b: f64) -> Result<f64, ToolError> {
    finite(a + b)
}

/// Subtracts `b` from `a`.
///
/// # Arguments
/// * `a` - The minuend.
/// * `b` - The subtrahend.
#[tool(read_only, idempotent, open_world = false)]
pub async fn subtract(a: f64, b: f64) -> Result<f64, ToolError> {
    finite(a - b)
}

/// Multiplies two numbers.
///
/// # Arguments
/// * `a` - The first factor.
/// * `b` - The second factor.
#[tool(read_only, idempotent, open_world = false)]
pub async fn multiply(a: f64, b: f64) -> Result<f64, ToolError> {
    finite(a * b)
}

/// Divides `a` by `b`. Fails when `b` is zero.
///
/// # Arguments
/// * `a` - The dividend.
/// * `b` - The divisor; must not be zero.
#[tool(read_only, idempotent, open_world = false)]
pub async fn divide(a: f64, b: f64) -> Result<f64, ToolError> {
    if b == 0.0 {
        return Err(ToolError::user("division by zero: `b` must not be 0"));
    }
    finite(a / b)
}

/// Descriptive statistics of a list of numbers.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Statistics {
    /// How many values were given.
    pub count: usize,
    /// Sum of the values.
    pub sum: f64,
    /// Arithmetic mean.
    pub mean: f64,
    /// Smallest value.
    pub min: f64,
    /// Largest value.
    pub max: f64,
}

/// Computes count, sum, mean, minimum and maximum of a list of numbers.
///
/// # Arguments
/// * `values` - The numbers to summarize (at least one).
#[tool(read_only, idempotent, open_world = false)]
pub async fn statistics(values: Vec<f64>) -> Result<Json<Statistics>, ToolError> {
    let (Some(&first), count) = (values.first(), values.len()) else {
        return Err(ToolError::user("`values` must contain at least one number"));
    };
    let sum = finite(values.iter().sum())?;
    let min = values.iter().copied().fold(first, f64::min);
    let max = values.iter().copied().fold(first, f64::max);
    let mean = finite(sum / f64::from(u32::try_from(count).unwrap_or(u32::MAX)))?;
    Ok(Json(Statistics {
        count,
        sum,
        mean,
        min,
        max,
    }))
}

/// The calculator server builder, with every tool registered.
pub fn builder() -> ServerBuilder {
    Server::builder("calculadora", env!("CARGO_PKG_VERSION"))
        .title("Calculadora")
        .instructions("Arithmetic on floating-point numbers. Use `statistics` to summarize a list.")
        .tool::<add>()
        .tool::<subtract>()
        .tool::<multiply>()
        .tool::<divide>()
        .tool::<statistics>()
}

/// Builds the calculator server.
///
/// # Errors
///
/// Fails only if the tool definitions are invalid.
pub fn server() -> Result<Server, BuildError> {
    builder().build()
}
