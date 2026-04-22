use ed25519_dalek::{SigningKey, Verifier};
use opolys_crypto::dilithium::{DilithiumKeypair, dilithium_verify};
use rand::rngs::OsRng;
use rand::TryRngCore;

pub struct HybridKeypair {
    pub classical: SigningKey,
    pub quantum: DilithiumKeypair,
}

impl HybridKeypair {
    pub fn generate() -> Self {
        let mut rng = OsRng;
        let mut seed = [0u8; 32];
        rng.try_fill_bytes(&mut seed).expect("Random generation failed");
        let classical = SigningKey::from_bytes(&seed);
        let quantum = DilithiumKeypair::generate();
        Self { classical, quantum }
    }

    pub fn sign_classical(&self, message: &[u8]) -> Vec<u8> {
        use ed25519_dalek::Signer;
        let sig: ed25519_dalek::Signature = self.classical.sign(message);
        sig.to_bytes().to_vec()
    }

    pub fn sign_quantum(&self, message: &[u8]) -> Vec<u8> {
        self.quantum.sign(message).to_vec()
    }

    pub fn verify_hybrid(
        classical_pk: &[u8],
        quantum_pk: &[u8],
        message: &[u8],
        classical_sig: &[u8],
        quantum_sig: &[u8],
    ) -> bool {
        let classical_ok = if classical_pk.len() == 32 && classical_sig.len() == 64 {
            if let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(
                <&[u8; 32]>::try_from(classical_pk).expect("Invalid pk len")
            ) {
                let sig_bytes: [u8; 64] = classical_sig.try_into().unwrap_or([0u8; 64]);
                let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
                vk.verify(message, &sig).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        let quantum_ok = dilithium_verify(quantum_sig, message, quantum_pk);

        classical_ok && quantum_ok
    }
}