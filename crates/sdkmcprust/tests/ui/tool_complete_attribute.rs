use sdkmcprust::tool;

#[tool]
fn echo(#[complete(values = ["a"])] text: String) -> String {
    text
}

fn main() {}
