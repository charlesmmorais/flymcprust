use flymcprust::resource;

#[resource("config://app", cache = "shared")]
fn config() -> &'static str {
    "{}"
}

fn main() {}
