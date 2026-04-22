

pub const DILITHIUM_PUBLIC_KEY_BYTES: usize = pqc_dilithium::PUBLICKEYBYTES;
pub const DILITHIUM_SECRET_KEY_BYTES: usize = pqc_dilithium::SECRETKEYBYTES;
pub const DILITHIUM_SIGNBYTES: usize = pqc_dilithium::SIGNBYTES;

pub struct DilithiumKeypair {
    inner: pqc_dilithium::Keypair,
}

impl DilithiumKeypair {
    pub fn generate() -> Self {
        DilithiumKeypair {
            inner: pqc_dilithium::Keypair::generate(),
        }
    }

    pub fn public_key(&self) -> &[u8] {
        &self.inner.public
    }

    pub fn secret_key(&self) -> &[u8] {
        self.inner.expose_secret()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; DILITHIUM_SIGNBYTES] {
        self.inner.sign(message)
    }
}

pub fn dilithium_verify(signature: &[u8], message: &[u8], public_key: &[u8]) -> bool {
    pqc_dilithium::verify(signature, message, public_key).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair() {
        let kp = DilithiumKeypair::generate();
        assert_eq!(kp.public_key().len(), DILITHIUM_PUBLIC_KEY_BYTES);
        assert_eq!(kp.secret_key().len(), DILITHIUM_SECRET_KEY_BYTES);
    }

    #[test]
    fn sign_and_verify() {
        let kp = DilithiumKeypair::generate();
        let msg = b"test message";
        let sig = kp.sign(msg);
        assert!(dilithium_verify(&sig, msg, kp.public_key()));
    }

    #[test]
    fn verify_wrong_message_fails() {
        let kp = DilithiumKeypair::generate();
        let sig = kp.sign(b"original");
        assert!(!dilithium_verify(&sig, b"tampered", kp.public_key()));
    }
}