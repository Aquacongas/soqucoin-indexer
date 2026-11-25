use super::{sha256, Wallet};
use anyhow::{ensure, Context, Result};
use zeroize::{Zeroize, Zeroizing};

// BIP39 English word list, https://github.com/bitcoin/bips/blob/master/bip-0039/english.txt
const ENGLISH: &str = include_str!("bip39-english.txt");
const WORD_COUNT: usize = 24;

pub(super) fn encode(entropy: &[u8; 32]) -> String {
    let words: Vec<_> = ENGLISH.lines().collect();
    let mut payload = Zeroizing::new([0u8; 33]);
    payload[..32].copy_from_slice(entropy);
    payload[32] = sha256(entropy)[0];
    let mut phrase = String::new();
    for word in 0..WORD_COUNT {
        let mut index = 0usize;
        for bit in word * 11..(word + 1) * 11 {
            index = (index << 1) | usize::from((payload[bit / 8] >> (7 - bit % 8)) & 1);
        }
        if word > 0 {
            phrase.push(' ');
        }
        phrase.push_str(words[index]);
    }
    phrase
}

pub(super) fn decode(phrase: &str) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(phrase.len() <= 1024, "Enter a 24-word recovery phrase");
    let words: Vec<_> = ENGLISH.lines().collect();
    let mut input = phrase.split_whitespace();
    let mut payload = Zeroizing::new([0u8; 33]);
    for word in 0..WORD_COUNT {
        let value = input.next().context("Enter a 24-word recovery phrase")?;
        let index = words
            .binary_search(&value)
            .ok()
            .context("Unknown recovery word")?;
        for offset in 0..11 {
            let bit = word * 11 + offset;
            payload[bit / 8] |= (((index >> (10 - offset)) & 1) as u8) << (7 - bit % 8);
        }
    }
    ensure!(input.next().is_none(), "Enter a 24-word recovery phrase");
    let mut entropy = Zeroizing::new([0u8; 32]);
    entropy.copy_from_slice(&payload[..32]);
    ensure!(
        payload[32] == sha256(entropy.as_ref())[0],
        "Invalid recovery phrase checksum"
    );
    Ok(entropy)
}

// BIP39 encodes the 256-bit entropy. SOQ derives the ML-DSA-44 key seed directly
// from that entropy with a fixed domain and counter. The counter skips keys
// with the public-key marker reserved by Soqucoin consensus.
pub(super) fn derive_wallet(entropy: &[u8; 32]) -> Result<Wallet> {
    let mut material = Zeroizing::new(b"SOQ.MARKET/ML-DSA-44/recovery/v1\0".to_vec());
    material.extend_from_slice(entropy);
    let counter_offset = material.len();
    for counter in 0..=u32::MAX {
        material.extend_from_slice(&counter.to_be_bytes());
        let mut seed = sha256(&material);
        let wallet = Wallet::from_seed(seed);
        seed.zeroize();
        material.truncate(counter_offset);
        if let Ok(mut wallet) = wallet {
            wallet.recovery_entropy = Some(Zeroizing::new(*entropy));
            return Ok(wallet);
        }
    }
    anyhow::bail!("Unable to derive a supported wallet key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bip39_checksum_and_soq_wallet_round_trip() {
        let entropy = [0u8; 32];
        let phrase = format!("{}art", "abandon ".repeat(23));
        assert_eq!(encode(&entropy), phrase);
        assert_eq!(*decode(&format!("  {}\n", phrase)).unwrap(), entropy);
        assert!(decode(&"abandon ".repeat(24)).is_err());
        assert!(decode("abandon").is_err());
        let original = derive_wallet(&entropy).unwrap();
        let restored = derive_wallet(&decode(&phrase).unwrap()).unwrap();
        assert_eq!(
            original.wif(158),
            "QTqtZjZzSguz2R7fgk9QHiMa4WYKi6j6Cko6C4oCbJ8SMbTzW1c4"
        );
        assert_eq!(original.wif(158), restored.wif(158));
        assert_eq!(
            original.receive_address("sq").unwrap(),
            restored.receive_address("sq").unwrap()
        );
    }
}
