use flymcprust::resource;

#[resource(template = "users://{id}")]
fn user(name: String) -> String {
    name
}

fn main() {}
