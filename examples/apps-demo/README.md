# MCP Apps demo

An MCP server whose tool opens an interactive page in the host: a price
survey shown as a table where the user excludes quotes and recomputes the
reference price.

| Piece | Where | What it does |
| --- | --- | --- |
| `price_table` | [src/lib.rs](src/lib.rs) | The interface: [ui/price-table.html](ui/price-table.html) served at `ui://apps-demo/price-table` as an MCP App |
| `price_survey` | [src/lib.rs](src/lib.rs) | The tool the model calls; its result opens the table (`ui = ".."`) |
| `reference_price` | [src/lib.rs](src/lib.rs) | Called by the page's "Recompute" button (`visibility = ["app"]`) |

The page loads the official `App` helper from `cdn.jsdelivr.net`, which the
resource declares in its CSP (`csp_resources`); the host's sandbox blocks
every other origin. To avoid the CDN, bundle the page into one file (for
example with `vite-plugin-singlefile`) and drop `csp_resources`.

## Run it

```bash
cargo run -p apps-demo
```

The server listens on `http://127.0.0.1:3001/mcp`.

## See it in a host

The quickest way is the test host of the official
[ext-apps](https://github.com/modelcontextprotocol/ext-apps) repository,
which connects to `http://localhost:3001/mcp` by default:

```bash
git clone https://github.com/modelcontextprotocol/ext-apps.git
```

In `ext-apps/examples/basic-host`:

```bash
npm install
```

```bash
npx cross-env NODE_ENV=development npm run build
```

```bash
npx tsx serve.ts
```

Open <http://localhost:8080>, pick `price_survey`, enter
`{"item": "A4 paper, 500 sheets"}` and click **Call Tool**: the table
appears. Untick a quote and click **Recompute**: the page calls
`reference_price` on this server and shows the new median.

To use it from Claude, the server must be reachable from the internet (for
example through a tunnel) and added as a custom connector. Then list the
tunnel's host name with `HttpConfig::allowed_hosts` (only `localhost` is
accepted by default) and require OAuth with `HttpConfig::oauth`.
