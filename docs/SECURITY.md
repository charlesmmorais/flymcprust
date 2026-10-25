# Security model

This document describes what SDKMCPRust defends against, how, and what
remains the responsibility of applications built with it. It also records
the findings of each security review. To report a vulnerability, see
[SECURITY.md](../SECURITY.md).

## 1. Scope and trust boundaries

SDKMCPRust is a library for MCP **servers** (spec revision 2026-07-28). It
receives JSON-RPC messages from clients over stdio or Streamable HTTP and
runs handler code (tools, resources, prompts) written by the application.

| Party | Trust |
| --- | --- |
| Application code (handlers, configuration) | Trusted. The SDK isolates its failures but does not sandbox it. |
| MCP client | **Untrusted.** Every byte of every message is attacker-controlled: arguments, URIs, `_meta`, `inputResponses`, `requestState`, headers. |
| Web pages in the user's browser | **Untrusted.** They can reach a local HTTP server (CSRF, DNS rebinding). |
| Network peers | **Untrusted.** They can open connections, send slowly, or send nothing. |
| stdio parent process | Trusted to launch the server; its messages are still validated like any client's. |

Out of scope: TLS termination and authentication (OAuth is a planned phase;
until then, put a reverse proxy with TLS and authentication in front of any
non-local deployment), sandboxing of handler code, and side channels of the
host.

## 2. Threats and controls

| Threat | Controls (all on by default) |
| --- | --- |
| Memory or CPU exhaustion by a single message | Size limit (4 MiB) checked before parsing; nesting depth (64) checked by a linear pre-scan before `serde`; duplicate JSON keys rejected; URIs over 8 KiB never match a template. |
| Exhaustion by many requests | Concurrent handler limit (64); per-handler timeout (30 s); HTTP: at most 256 requests being read or processed (`503` beyond), at most 1024 connections, header timeout (10 s, from connection open), body timeout (30 s, `408`). stdio: backpressure stops reading when 64 requests are in flight. |
| Crash or hang through handler bugs | No `unsafe`; `unwrap`/`expect`/`panic!`/indexing denied by lints outside tests; every request runs under `catch_unwind` (including synchronous argument parsers and completion providers) and panics become `-32603`. |
| Information disclosure through errors | Internal errors are logged and replaced by generic messages; client input echoed in errors is shortened (quoted fragments to 32 characters, messages to 512). |
| Browser attacks on a local server (CSRF, DNS rebinding) | `Origin` and `Host` allowlists default to localhost; `Content-Type: application/json` required (no CORS-simple requests); CORS preflight only for allowed origins; no credentials in CORS; `X-Content-Type-Options: nosniff`. |
| Header/body desynchronization (SEP-2243) | `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` and every `Mcp-Param-*` must match the body exactly (integers compared exactly, base64 sentinel validated); duplicate MCP headers rejected. |
| Path traversal through resource URIs | Decoded `{var}`, `{/var}` and query values never contain `/`, `\` or control characters; `security::confine_path` confines `{+path}` values (and any file argument) to a root, after resolving symbolic links. |
| Forged legacy sessions or answers (2025-11-25 clients) | `MCP-Session-Id` tokens are HMAC-signed (separate signing context from `requestState`), expire after 24 h and carry a random nonce, so they are unique and cannot be forged or altered; invalid ones get `404`. Server-to-client requests use random 128-bit ids bound to the session that caused them, so another client cannot answer them. |
| Forged or replayed `requestState` (MRTR) | HMAC-SHA256 with a 256-bit random key (or a configured one of at least 32 bytes), verified in constant time before parsing; bound to method, target, argument digest and expiry (10 min); only answers the handler actually read are carried forward. |
| Requests for input the client cannot serve | Client capabilities checked before any input request (`-32021`). |
| Supply chain | `cargo-deny` (licenses, advisories, bans, sources) and `cargo-audit` in CI; protocol schema pinned by SHA-256. |

## 3. Responsibilities of applications

The SDK cannot enforce these; applications must:

1. **Authenticate remote access.** The HTTP transport has no authentication
   yet. Keep it on loopback, or front it with TLS and authentication. Do not
   use `AllowedOrigins::Any` without authentication.
2. **Confine files.** Pass any client-provided path through
   `security::confine_path`.
3. **Make code before `ctx.elicit` idempotent.** Handlers rerun from the
   start on every MRTR round.
4. **Share the `requestState` key across instances** with
   `request_state_key`, keep it secret, and rotate it like any signing key.
   Rotation invalidates pending input requests.
5. **Keep completion providers fast.** They run synchronously, without the
   handler timeout.
6. **Treat answers as untrusted.** Elicitation, sampling and roots answers
   come from the client.
7. **Mind what you log.** Logs may contain client input.

## 4. Known limitations

- Legacy sessions cannot be revoked before they expire (`DELETE` gets
  `405`); rotating the key ends all of them. A legacy request without a session
  is served with no client capabilities.

- `requestState` is not yet bound to the authenticated principal (a spec
  SHOULD); this comes with OAuth. Within its 10-minute lifetime, a state can
  be replayed by whoever holds it, on the same request.
- The built-in HTTP server speaks HTTP/1.1 only. For HTTP/2, mount
  `StreamableHttpService` in another server and configure its timeouts.
- Signing keys are not zeroized in memory.

## 5. Verification

- Regression tests for every finding: `crates/sdkmcprust/tests/security.rs`
  and `crates/sdkmcprust-core/tests/http.rs`.
- Fuzz targets (60 s each in CI): `parse_message`, `handle_message`,
  `match_uri` (asserts the separator and control-character guarantees).
- The official conformance suite runs in CI, including the tampered-state
  and DNS-rebinding scenarios.

## 6. Review log

### 2026-10-25: full review of the SDK

Reviewed: JSON-RPC parsing and limits, stdio and HTTP transports, dispatch,
error mapping, tools, resources, prompts, completion, MRTR and
`requestState`, URI templates, macros. Every finding was reproduced by a
failing test before the fix.

| # | Severity | Finding | Fix |
| --- | --- | --- | --- |
| 1 | High | `{var}` values were percent-decoded after the separator check, so `..%2F..%2Fetc%2Fpasswd` (or a raw `\` on Windows) reached handlers relying on the documented "no `/`" guarantee: path traversal. | Decoded values are re-checked: no `/`, `\` or control characters (query and `{/var}` values too; `{+var}` no control characters). |
| 2 | Medium | A panic in synchronous code (a `FromStr` argument parser, a completion provider) escaped request handling: over stdio the request never got an answer and its id stayed reserved; over HTTP the connection dropped. | `Server::handle` runs dispatch under `catch_unwind`; panics become `-32603`. |
| 3 | Medium | New connections that sent nothing were never closed: automatic HTTP/2 detection waits for the first bytes without a deadline. Enough idle sockets exhaust file descriptors. | Built-in server is HTTP/1.1, whose header timeout starts when the connection opens; connection cap added. |
| 4 | Medium | Request bodies had no read deadline and the number of bodies being read was unbounded (the handler limit only applies after reading): slow or parallel uploads could hold up to 4 MiB each indefinitely. | Body timeout (`408`) and in-flight request limit (`503`, `Retry-After`). |
| 5 | Low | Answers the handler never asked for were signed into `requestState`, letting a client make the server echo up to 4 MiB of its own data on every round (267 KB in the test). | Only answers the handler read are carried forward. |
| 6 | Low | Deserialization errors quoted the offending client value in full (100 KB echo in the test). | Quoted fragments shortened and messages capped. |
| 7 | Low | `Mcp-Param-*` integers were compared through `f64`, so values above 2^53 that differ could be accepted as equal, a header/body desynchronization. | Exact integer comparison. |
| 8 | Improvement | No safe way to map a client path to a file, while the spec requires preventing traversal. | `security::confine_path`. |
| 9 | Hardening | JSON responses lacked `X-Content-Type-Options`. | `nosniff` added. |
