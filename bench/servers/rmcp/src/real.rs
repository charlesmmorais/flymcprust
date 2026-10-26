//! Real-world tools of the rmcp server: a complex order payload, an
//! upstream call, and (in `many.rs`) 200 generated tools.
#![allow(dead_code)] // Fields are deserialized (the measured work), not read.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Address {
    street: String,
    city: String,
    postal_code: String,
    country: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Customer {
    id: u64,
    name: String,
    email: String,
    address: Address,
    tags: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Physical,
    Digital,
    Service,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Item {
    sku: String,
    quantity: u32,
    unit_price: f64,
    kind: Kind,
    attributes: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct Order {
    id: String,
    customer: Customer,
    items: Vec<Item>,
    notes: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OrderArgs {
    order: Order,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct OrderSummary {
    id: String,
    customer: String,
    items: usize,
    digital: usize,
    total: f64,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FetchArgs {
    id: u64,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AddArgs {
    a: f64,
    b: f64,
}

#[derive(Clone)]
pub struct RealTools {
    router: ToolRouter<RealTools>,
}

impl RealTools {
    pub fn new() -> Self {
        Self {
            router: Self::real_router() + Self::many_router(),
        }
    }
}

#[tool_router(router = real_router, vis = "pub")]
impl RealTools {
    #[tool(description = "Adds two numbers.")]
    fn add(&self, Parameters(AddArgs { a, b }): Parameters<AddArgs>) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Validates an order and returns its summary.")]
    fn order(&self, Parameters(OrderArgs { order }): Parameters<OrderArgs>) -> Json<OrderSummary> {
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

    #[tool(description = "Fetches a record from the upstream service.")]
    async fn fetch(&self, Parameters(FetchArgs { id }): Parameters<FetchArgs>) -> String {
        crate::upstream::get(id)
            .await
            .unwrap_or_else(|_| "upstream unavailable".to_owned())
    }
}

#[tool_handler(router = self.router)]
impl ServerHandler for RealTools {}
