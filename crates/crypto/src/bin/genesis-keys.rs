use ed25519_dalek::SigningKey;
use rand::TryRngCore;
use rand::rngs::OsRng;

fn main() {
    for i in 0..3 {
        let mut seed = [0u8; 32];
        OsRng.try_fill_bytes(&mut seed).unwrap();
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let pk_bytes = *verifying_key.as_bytes();
        let object_id = opolys_crypto::ed25519_public_key_to_object_id(&pk_bytes);

        println!("// Account {}", i);
        println!(r#"seed_{}: "{}","#, i, hex::encode(seed));
        println!(r#"public_key_{}: "{}","#, i, hex::encode(pk_bytes));
        println!(r#"object_id_{}: "{}","#, i, object_id.to_hex());
        println!();
    }
}
