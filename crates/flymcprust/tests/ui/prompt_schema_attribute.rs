use flymcprust::prompt;

#[prompt]
fn greet(#[serde(default)] name: String) -> String {
    name
}

fn main() {}
