use mcpsdkrust::tool;

#[tool(scopes = ["files write"])]
fn wipe() {}

#[tool(scopes = "files")]
fn wipe_all() {}

fn main() {}
