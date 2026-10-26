//! An MCP server with an MCP Apps interface: a price survey shown as an
//! interactive table.
//!
//! - `price_survey` (for the model) returns the quotes collected for an item
//!   and shows them in the `ui://apps-demo/price-table` interface;
//! - the table lets the user exclude quotes and calls `reference_price`
//!   (visible to the interface only) to recompute the reference price.
//!
//! The quotes are made up: a real server would query a database or an
//! external service.

use flymcprust::prelude::*;
use serde::Serialize;

/// The interface page. It loads the official `App` helper from a CDN, so the
/// resource declares that origin; anything else is blocked by the host's
/// sandbox.
#[resource(
    "ui://apps-demo/price-table",
    name = "price_table",
    title = "Price table",
    description = "Interactive table of the quotes collected for an item.",
    app,
    csp_resources = ["https://cdn.jsdelivr.net"],
    prefers_border = true
)]
fn price_table() -> &'static str {
    include_str!("../ui/price-table.html")
}

/// One quote.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Quote {
    /// Where the price comes from.
    pub source: String,
    /// Unit price, in reais.
    pub price: f64,
}

/// The quotes collected for an item, with the reference price.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Survey {
    /// The item searched.
    pub item: String,
    /// The quotes found.
    pub quotes: Vec<Quote>,
    /// Median of the quotes.
    pub reference_price: f64,
}

/// The reference price of a set of quotes.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Reference {
    /// Median of the quotes.
    pub median: f64,
    /// Mean of the quotes.
    pub mean: f64,
    /// How many quotes were used.
    pub count: usize,
}

fn median(prices: &[f64]) -> f64 {
    let mut sorted = prices.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    match (sorted.get(mid.wrapping_sub(1)), sorted.get(mid)) {
        (Some(a), Some(b)) if sorted.len().is_multiple_of(2) => (a + b) / 2.0,
        (_, Some(m)) => *m,
        _ => 0.0,
    }
}

/// Searches the quotes of an item and shows them in an interactive table,
/// with the reference price (median).
///
/// # Arguments
///
/// * `item` - Description of the item, e.g. "A4 paper, 500 sheets".
#[tool(read_only, ui = "ui://apps-demo/price-table")]
fn price_survey(item: String) -> Json<Survey> {
    // Made-up quotes, deterministic for the item.
    let seed = item
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let base = 20.0 + f64::from(seed % 400) / 10.0;
    let sources = [
        "Supplier A",
        "Supplier B",
        "Public contract 2026",
        "Online store",
        "Supplier C",
    ];
    let quotes: Vec<Quote> = sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            // One deliberately high quote, which the user may exclude.
            let factor = if i == 3 { 1.9 } else { 0.9 + (i as f64) * 0.06 };
            Quote {
                source: (*source).to_owned(),
                price: (base * factor * 100.0).round() / 100.0,
            }
        })
        .collect();
    let prices: Vec<f64> = quotes.iter().map(|q| q.price).collect();
    Json(Survey {
        item,
        reference_price: median(&prices),
        quotes,
    })
}

/// Recomputes the reference price of the quotes kept in the table. Only the
/// interface calls it; the model does not see it.
///
/// # Arguments
///
/// * `prices` - The unit prices kept by the user.
#[tool(read_only, visibility = ["app"])]
fn reference_price(prices: Vec<f64>) -> Result<Json<Reference>, ToolError> {
    if prices.is_empty() {
        return Err(ToolError::user("Keep at least one quote."));
    }
    Ok(Json(Reference {
        median: median(&prices),
        mean: prices.iter().sum::<f64>() / prices.len() as f64,
        count: prices.len(),
    }))
}

/// The server.
///
/// # Errors
///
/// Never in practice: the definitions are valid.
pub fn server() -> Result<Server, flymcprust::BuildError> {
    Server::builder("apps-demo", env!("CARGO_PKG_VERSION"))
        .instructions("Use price_survey to show the quotes of an item in an interactive table.")
        .resource::<price_table>()
        .tool::<price_survey>()
        .tool::<reference_price>()
        .build()
}
