//! One-off local key generation for manifest signing — run once, save
//! the private key outside the repo, embed the public key in agent-bin.
//! Not part of the crate's real API surface, deliberately an `examples/`
//! throwaway, never shipped in the built binary.
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    println!("PRIVATE_KEY_HEX={}", to_hex(&signing_key.to_bytes()));
    println!("PUBLIC_KEY_HEX={}", to_hex(&verifying_key.to_bytes()));
}
