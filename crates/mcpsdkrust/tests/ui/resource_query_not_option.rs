use mcpsdkrust::resource;

#[resource(template = "search://items{?q}")]
fn search(q: String) -> String {
    q
}

fn main() {}
