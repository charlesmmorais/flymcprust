use flymcprust::resource;

#[resource(template = "users://{id}/{field}")]
fn user(id: u64) -> String {
    id.to_string()
}

fn main() {}
