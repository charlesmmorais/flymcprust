//! Real-world tools of the FlyMCPRust server: a complex order payload, an
//! upstream call, and (in `many.rs`) 200 generated tools.
#![allow(dead_code)] // Fields are deserialized (the measured work), not read.

use flymcprust::prelude::*;
use serde::{Deserialize, Serialize};

/// A postal address.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Address {
    street: String,
    city: String,
    postal_code: String,
    country: String,
}

/// The customer of an order.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Customer {
    id: u64,
    name: String,
    email: String,
    address: Address,
    tags: Vec<String>,
}

/// What an item is.
#[derive(Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Shipped goods.
    Physical,
    /// Downloads and licenses.
    Digital,
    /// Work by the hour.
    Service,
}

/// An order line.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Item {
    sku: String,
    quantity: u32,
    unit_price: f64,
    kind: Kind,
    attributes: Vec<String>,
}

/// An order.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Order {
    id: String,
    customer: Customer,
    items: Vec<Item>,
    notes: Option<String>,
}

/// The summary of an order.
#[derive(Serialize, schemars::JsonSchema)]
pub struct OrderSummary {
    id: String,
    customer: String,
    items: usize,
    digital: usize,
    total: f64,
}

/// Validates an order and returns its summary.
///
/// # Arguments
///
/// * `order` - The order to check.
#[tool]
pub fn order(order: Order) -> Json<OrderSummary> {
    let _ = (
        &order.notes,
        &order.customer.id,
        &order.customer.address.street,
    );
    Json(OrderSummary {
        id: order.id,
        customer: order.customer.name,
        items: order.items.len(),
        digital: order
            .items
            .iter()
            .filter(|i| i.kind == Kind::Digital)
            .count(),
        total: order
            .items
            .iter()
            .map(|i| f64::from(i.quantity) * i.unit_price)
            .sum(),
    })
}

/// Fetches a record from the upstream service.
///
/// # Arguments
///
/// * `id` - The record id.
#[tool(read_only)]
pub async fn fetch(id: u64) -> String {
    crate::upstream::get(id)
        .await
        .unwrap_or_else(|_| "upstream unavailable".to_owned())
}
