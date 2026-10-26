# From zero to an MCP server in Rust in 5 minutes

You will build a small [Model Context Protocol](https://modelcontextprotocol.io)
server with a tool, a resource and a prompt, try it with the MCP Inspector,
and test it. You need Rust (stable) and, for the Inspector, Node.js.

Every step below was run as written on a fresh project.

## 1. Create the project (1 minute)

```bash
cargo new hello-mcp
```

```bash
cd hello-mcp
```

```bash
cargo add flymcprust --git https://github.com/charlesmmorais/flymcprust --features stdio,logging
```

```bash
cargo add tokio --features macros,rt-multi-thread
```

The SDK is not on crates.io yet; once it is, the third command becomes
`cargo add flymcprust --features stdio,logging`.

## 2. Write a tool (1 minute)

Replace `src/main.rs`:

```rust
use flymcprust::prelude::*;

/// Says hello to someone.
///
/// # Arguments
/// * `name` - Who to greet.
#[tool]
fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    flymcprust::logging::init();
    Server::builder("hello-mcp", "0.1.0")
        .tool::<hello>()
        .build()?
        .serve_stdio()
        .await?;
    Ok(())
}
```

That is a complete server. The doc comment becomes the tool's description,
`name: String` becomes its JSON Schema, and `logging::init()` sends the log
to stderr (stdout carries the protocol).

```bash
cargo build
```

If you run it directly (`cargo run`), it explains that it waits for an MCP
client on stdin. That is expected; press Ctrl+C.

## 3. Try it (1 minute)

The [MCP Inspector](https://github.com/modelcontextprotocol/inspector) can
call the tool from the command line:

```bash
npx @modelcontextprotocol/inspector --cli target/debug/hello-mcp --method tools/call --tool-name hello --tool-args-json '{"name":"Ada"}'
```

```json
{
  "content": [{ "type": "text", "text": "Hello, Ada!" }]
}
```

(The output also carries `_meta` with your server's name and version.)

Your server speaks MCP 2026-07-28 and, for clients that have not moved yet,
2025-11-25 and 2025-06-18 (the Inspector's default); nothing to configure.
On Windows, use `target\debug\hello-mcp.exe`.
Run `npx @modelcontextprotocol/inspector` without `--cli` for the web UI.

## 4. Add a resource, a prompt and a question for the user (1 minute)

Questions to the user need two more crates:

```bash
cargo add serde --features derive
```

```bash
cargo add schemars
```

Then extend `src/main.rs`:

```rust
use flymcprust::prelude::*;
use flymcprust::BuildError;

/// Says hello to someone.
///
/// # Arguments
/// * `name` - Who to greet.
#[tool]
fn hello(name: String) -> String {
    format!("Hello, {name}!")
}

/// The greeting style of this server.
#[resource("config://style", mime_type = "text/plain")]
fn style() -> &'static str {
    "friendly"
}

/// Writes a welcome message.
///
/// # Arguments
/// * `name` - Who to welcome.
#[prompt]
fn welcome(name: String) -> String {
    format!("Write a warm, two-sentence welcome message for {name}.")
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Confirm {
    /// Send the greeting?
    confirmed: bool,
}

/// Greets someone, after asking the user to confirm.
#[tool]
async fn careful_hello(ctx: Ctx, name: String) -> Result<String, ToolError> {
    let answer: Confirm = ctx.elicit("confirm", format!("Greet {name}?")).await?;
    if answer.confirmed {
        Ok(format!("Hello, {name}!"))
    } else {
        Err(ToolError::user("The user chose not to send the greeting."))
    }
}

fn server() -> Result<Server, BuildError> {
    Server::builder("hello-mcp", "0.1.0")
        .tool::<hello>()
        .tool::<careful_hello>()
        .resource::<style>()
        .prompt::<welcome>()
        .build()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    flymcprust::logging::init();
    server()?.serve_stdio().await?;
    Ok(())
}
```

Rebuild with `cargo build`, then read the resource and get the prompt:

```bash
npx @modelcontextprotocol/inspector --cli target/debug/hello-mcp --method resources/read --uri config://style
```

```bash
npx @modelcontextprotocol/inspector --cli target/debug/hello-mcp --method prompts/get --prompt-name welcome --prompt-args name=Ada
```

`ctx.elicit` asks the user through the client: the client shows a form
generated from `Confirm` and retries the call with the answer. The SDK
handles the round trips. Clients must support elicitation; the Inspector's
command line does not, and your server's log says so:

```text
WARN a client that did not declare the needed capability called a handler that asks it for input; it received -32021
  = help: check ctx.client_capabilities() before asking if the handler can work without the answer
```

## 5. Test it (1 minute)

```bash
cargo add flymcprust --dev --git https://github.com/charlesmmorais/flymcprust --features test-client
```

```bash
cargo add serde_json --dev
```

Append to `src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use flymcprust::testing::TestClient;
    use serde_json::json;

    #[tokio::test]
    async fn hello_greets() {
        let client = TestClient::new(server().unwrap());
        let result = client.call_tool("hello", json!({ "name": "Ada" })).await.unwrap();
        let result = serde_json::to_value(result).unwrap();
        assert_eq!(result["content"][0]["text"], "Hello, Ada!");
    }
}
```

```bash
cargo test
```

`TestClient` runs the server in process through the real wire format: no
process, no network.

## Connect it to an MCP client

Build a release binary:

```bash
cargo build --release
```

Most clients take a command to start. For example, in Claude Desktop's
`claude_desktop_config.json` or a project's `.mcp.json` (Claude Code):

```json
{
  "mcpServers": {
    "hello": { "command": "/absolute/path/to/hello-mcp/target/release/hello-mcp" }
  }
}
```

Clients on MCP 2026-07-28, 2025-11-25 or 2025-06-18 all work; the SDK adapts
the protocol, including `ctx.elicit`, which older clients receive as a request
from the server.

## When something goes wrong

Mistakes are reported with the fix, never as bare errors:

- **In the code:** misusing the macros is a compile error at the exact
  spot (a borrowed parameter, an unknown argument, an invalid URI template).
- **At startup:** `build()` explains what to change. Registering a prompt
  twice prints:

  ```text
  Error: duplicate prompt name `welcome`
    = help: rename one of the prompts with #[prompt(name = "...")]
  ```

- **At run time:** the log says what was rejected and how to allow it (a
  host name, a larger message, a longer timeout). Clients only get short,
  generic errors, so nothing internal leaks.

## Next steps

- Serve over HTTP: add the `http` feature and call
  `.serve_http("127.0.0.1:8000")`. Remote access needs host names and
  authentication; see [SECURITY.md](SECURITY.md).
- Shared state (`State<T>`), structured output (`Json<T>`), URI templates,
  completion and limits: see the [README](../README.md).
