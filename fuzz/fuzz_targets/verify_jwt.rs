//! JWT verification must never panic, and must never accept a token that
//! the fixed test key did not sign, whatever the input.
#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use mcpsdkrust_core::TokenVerifier as _;
use mcpsdkrust_core::auth::JwtVerifier;

/// Public keys from the RFC 8037 and RFC 7517 examples. Their private
/// halves are published, but a fuzzer cannot compute signatures.
const JWKS: &str = r#"{"keys":[
  {"kty":"OKP","crv":"Ed25519","kid":"ed","x":"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"},
  {"kty":"EC","crv":"P-256","kid":"ec","x":"f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU","y":"x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"}
]}"#;

fn verifier() -> &'static JwtVerifier {
    static VERIFIER: OnceLock<JwtVerifier> = OnceLock::new();
    VERIFIER.get_or_init(|| {
        JwtVerifier::builder("https://auth.example.com", "https://mcp.example.com")
            .jwks(JWKS)
            .build()
            .expect("valid test configuration")
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(token) = std::str::from_utf8(data) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let result = runtime.block_on(verifier().verify(token));
    assert!(result.is_err(), "accepted a token without a valid signature");
});
