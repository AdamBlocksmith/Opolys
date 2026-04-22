use ed25519_dalek::{SigningKey, Signer, Verifier, VerifyingKey, Signature as DalekSignature};
use opolys_core::ObjectId;
use rand::rngs::OsRng;
use rand::TryRngCore;

fn hash_bytes_to_object_id(data: &[u8]) -> ObjectId {
    let hash = crate::hash::hash(data);
    ObjectId(hash)
}

pub fn ed25519_public_key_to_object_id(public_key: &[u8; 32]) -> ObjectId {
    hash_bytes_to_object_id(public_key)
}

pub fn verify_ed25519(
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> bool {
    if public_key.len() != 32 {
        return false;
    }
    let pk_bytes: [u8; 32] = match public_key.try_into() {
        Ok(b) => b,
        Err(_) => return false,
    };

    let verifying_key = match VerifyingKey::from_bytes(&pk_bytes) {
        Ok(vk) => vk,
        Err(_) => return false,
    };

    if signature.len() != 64 {
        return false;
    }

    let sig_bytes: [u8; 64] = match signature.try_into() {
        Ok(b) => b,
        Err(_) => return false,
    };

    let dalek_sig = DalekSignature::from_bytes(&sig_bytes);
    verifying_key.verify(message, &dalek_sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_keypair() -> (SigningKey, ObjectId) {
        let mut seed = [0u8; 32];
        OsRng.try_fill_bytes(&mut seed).unwrap();
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let object_id = ed25519_public_key_to_object_id(verifying_key.as_bytes());
        (signing_key, object_id)
    }

    #[test]
    fn sign_and_verify() {
        let (signing_key, _) = generate_keypair();
        let message = b"test message";
        let signature: DalekSignature = signing_key.sign(message);
        let verifying_key = signing_key.verifying_key();

        assert!(verify_ed25519(
            verifying_key.as_bytes(),
            message,
            &signature.to_bytes(),
        ));
    }

    #[test]
    fn verify_wrong_message_fails() {
        let (signing_key, _) = generate_keypair();
        let message = b"test message";
        let signature: DalekSignature = signing_key.sign(message);
        let verifying_key = signing_key.verifying_key();

        assert!(!verify_ed25519(
            verifying_key.as_bytes(),
            b"wrong message",
            &signature.to_bytes(),
        ));
    }

    #[test]
    fn object_id_from_public_key() {
        let (_, id1) = generate_keypair();
        let (_, id2) = generate_keypair();
        assert_ne!(id1, id2);
    }
}