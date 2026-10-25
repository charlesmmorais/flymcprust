use sdkmcprust::{resource, tool};

#[tool(ui = "https://example.com/view")]
fn not_a_ui_uri() {}

#[tool(visibility = ["everyone"])]
fn bad_visibility() {}

#[resource("ui://x/view", csp_connect = ["https://api.example.com"])]
fn settings_without_app() -> String {
    String::new()
}

#[resource(template = "ui://x/{id}", app)]
fn app_template(id: String) -> String {
    id
}

#[resource("ui://x/view", app, permissions = ["telepathy"])]
fn bad_permission() -> String {
    String::new()
}

#[resource("ui://x/view", app, mime_type = "text/html")]
fn app_with_mime() -> String {
    String::new()
}

fn main() {}
