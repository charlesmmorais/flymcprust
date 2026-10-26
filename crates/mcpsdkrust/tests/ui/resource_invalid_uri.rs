use mcpsdkrust::resource;

#[resource("not a uri")]
fn config() -> &'static str {
    "{}"
}

fn main() {}
