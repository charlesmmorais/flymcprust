use flymcprust::tool;

#[tool]
fn echo<T: ToString>(value: T) -> String {
    value.to_string()
}

fn main() {}
