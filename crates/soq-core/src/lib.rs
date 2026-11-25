//! Transparent SOQ witness-v1 transaction primitives. No consensus engine.
pub mod fees;
pub mod protocol;
use anyhow::{bail, ensure, Context, Result};
use bech32::{FromBase32, ToBase32, Variant};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}
pub fn hash256(data: &[u8]) -> [u8; 32] {
    sha256(&sha256(data))
}
pub fn hash_bytes(s: &str) -> Result<[u8; 32]> {
    hex::decode(s)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected 32-byte hash"))
}
pub fn display_hash(h: [u8; 32]) -> String {
    hex::encode(h.into_iter().rev().collect::<Vec<_>>())
}
pub fn wire_hash(h: &str) -> Result<[u8; 32]> {
    let mut h = hash_bytes(h)?;
    h.reverse();
    Ok(h)
}
pub fn compact(n: u64, out: &mut Vec<u8>) {
    match n {
        0..=252 => out.push(n as u8),
        253..=65535 => {
            out.push(253);
            out.extend((n as u16).to_le_bytes());
        }
        65536..=4294967295 => {
            out.push(254);
            out.extend((n as u32).to_le_bytes());
        }
        _ => {
            out.push(255);
            out.extend(n.to_le_bytes());
        }
    }
}
pub fn bytes(b: &[u8], out: &mut Vec<u8>) {
    compact(b.len() as u64, out);
    out.extend(b);
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutPoint {
    pub txid: String,
    pub vout: u32,
}
impl OutPoint {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        out.extend(wire_hash(&self.txid)?);
        out.extend(self.vout.to_le_bytes());
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Input {
    pub previous: OutPoint,
    pub script: Vec<u8>,
    pub sequence: u32,
    pub witness: Vec<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Output {
    pub value: u64,
    pub script: Vec<u8>,
}
impl Output {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend(self.value.to_le_bytes());
        bytes(&self.script, out);
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    pub version: i32,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub lock_time: u32,
}
impl Transaction {
    pub fn encode(&self, witness: bool) -> Result<Vec<u8>> {
        ensure!(!self.inputs.is_empty(), "transaction has no inputs");
        let mut out = self.version.to_le_bytes().to_vec();
        let wit = witness && self.inputs.iter().any(|i| !i.witness.is_empty());
        if wit {
            out.extend([0, 1]);
        }
        compact(self.inputs.len() as u64, &mut out);
        for i in &self.inputs {
            i.previous.encode(&mut out)?;
            bytes(&i.script, &mut out);
            out.extend(i.sequence.to_le_bytes());
        }
        compact(self.outputs.len() as u64, &mut out);
        for o in &self.outputs {
            o.encode(&mut out);
        }
        if wit {
            for i in &self.inputs {
                compact(i.witness.len() as u64, &mut out);
                for w in &i.witness {
                    bytes(w, &mut out);
                }
            }
        }
        out.extend(self.lock_time.to_le_bytes());
        Ok(out)
    }
    pub fn txid(&self) -> Result<String> {
        Ok(display_hash(hash256(&self.encode(false)?)))
    }
    /// BIP143 SIGHASH_ALL only, using the prevout's OP_1 <SHA256(pk)> script.
    pub fn sighash(&self, index: usize, prevout: &Output) -> Result<[u8; 32]> {
        self.sighash_type(index, prevout, 1)
    }
    /// Restricted to SIGHASH_ALL and SIGHASH_SINGLE|ANYONECANPAY used by offers.
    pub fn sighash_type(&self, index: usize, prevout: &Output, hash_type: u8) -> Result<[u8; 32]> {
        ensure!(
            hash_type == 1 || hash_type == 0x83,
            "unsupported sighash type"
        );
        let input = self.inputs.get(index).context("input index out of range")?;
        ensure!(
            prevout.script.len() == 34 && prevout.script[..2] == [0x51, 32],
            "only transparent witness v1 supported"
        );
        let mut prev = Vec::new();
        let mut seq = Vec::new();
        let mut outputs = Vec::new();
        for i in &self.inputs {
            i.previous.encode(&mut prev)?;
            seq.extend(i.sequence.to_le_bytes());
        }
        if hash_type == 0x83 {
            self.outputs
                .get(index)
                .context("SINGLE output missing")?
                .encode(&mut outputs);
        } else {
            for o in &self.outputs {
                o.encode(&mut outputs);
            }
        }
        let mut p = self.version.to_le_bytes().to_vec();
        p.extend(if hash_type == 0x83 {
            [0; 32]
        } else {
            hash256(&prev)
        });
        p.extend(if hash_type == 0x83 {
            [0; 32]
        } else {
            hash256(&seq)
        });
        input.previous.encode(&mut p)?;
        bytes(&prevout.script, &mut p);
        p.extend(prevout.value.to_le_bytes());
        p.extend(input.sequence.to_le_bytes());
        p.extend(hash256(&outputs));
        p.extend(self.lock_time.to_le_bytes());
        p.extend((hash_type as u32).to_le_bytes());
        Ok(hash256(&p))
    }
    pub fn decode(raw: &[u8]) -> Result<Self> {
        ensure!(raw.len() <= 2_000_000, "transaction too large");
        let mut r = Reader { raw, pos: 0 };
        let version = i32::from_le_bytes(r.take(4)?.try_into()?);
        let mut count = r.count(100_000)?;
        let witness = count == 0;
        if witness {
            ensure!(r.take(1)? == [1], "unknown witness flag");
            count = r.count(100_000)?;
        }
        ensure!(count > 0, "empty inputs");
        let mut inputs = Vec::new();
        for _ in 0..count {
            let h: [u8; 32] = r.take(32)?.try_into()?;
            let vout = u32::from_le_bytes(r.take(4)?.try_into()?);
            let script = r.blob()?;
            let sequence = u32::from_le_bytes(r.take(4)?.try_into()?);
            inputs.push(Input {
                previous: OutPoint {
                    txid: display_hash(h),
                    vout,
                },
                script,
                sequence,
                witness: vec![],
            });
        }
        let n = r.count(100_000)?;
        let mut outputs = Vec::new();
        for _ in 0..n {
            let value = u64::from_le_bytes(r.take(8)?.try_into()?);
            ensure!(value <= i64::MAX as u64, "negative output");
            outputs.push(Output {
                value,
                script: r.blob()?,
            });
        }
        if witness {
            for i in &mut inputs {
                let n = r.count(100_000)?;
                for _ in 0..n {
                    i.witness.push(r.blob()?);
                }
            }
            ensure!(
                inputs.iter().any(|i| !i.witness.is_empty()),
                "superfluous witness"
            );
        }
        let lock_time = u32::from_le_bytes(r.take(4)?.try_into()?);
        ensure!(r.pos == raw.len(), "trailing transaction data");
        Ok(Self {
            version,
            inputs,
            outputs,
            lock_time,
        })
    }
}
struct Reader<'a> {
    raw: &'a [u8],
    pos: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).context("length overflow")?;
        let b = self
            .raw
            .get(self.pos..end)
            .context("truncated transaction")?;
        self.pos = end;
        Ok(b)
    }
    fn count(&mut self, max: u64) -> Result<u64> {
        let first = self.take(1)?[0];
        let n = match first {
            253 => {
                let n = u16::from_le_bytes(self.take(2)?.try_into()?) as u64;
                ensure!(n >= 253, "noncanonical compact size");
                n
            }
            254 => {
                let n = u32::from_le_bytes(self.take(4)?.try_into()?) as u64;
                ensure!(n > 65535, "noncanonical compact size");
                n
            }
            255 => {
                let n = u64::from_le_bytes(self.take(8)?.try_into()?);
                ensure!(n > u32::MAX as u64, "noncanonical compact size");
                n
            }
            n => n as u64,
        };
        ensure!(n <= max, "length exceeds limit");
        Ok(n)
    }
    fn blob(&mut self) -> Result<Vec<u8>> {
        let n = self.count(2_000_000)? as usize;
        Ok(self.take(n)?.to_vec())
    }
}
pub fn address(public_key: &[u8], hrp: &str) -> Result<String> {
    ensure!(
        public_key.len() == 1312 && public_key[0] != 255,
        "invalid ML-DSA-44 key"
    );
    ensure!(["sq", "ssq"].contains(&hrp), "unknown network");
    let mut data = vec![bech32::u5::try_from_u8(1)?];
    data.extend(sha256(public_key).to_base32());
    Ok(bech32::encode(hrp, data, Variant::Bech32m)?)
}
pub fn address_script(address: &str, hrp: &str) -> Result<Vec<u8>> {
    let (prefix, data, variant) = bech32::decode(address)?;
    ensure!(
        prefix == hrp && variant == Variant::Bech32m && data.first().map(|d| d.to_u8()) == Some(1),
        "wrong network or address version"
    );
    let program = Vec::<u8>::from_base32(&data[1..])?;
    ensure!(program.len() == 32, "invalid address program");
    let mut s = vec![0x51, 32];
    s.extend(program);
    Ok(s)
}
pub fn script_address(script: &[u8], hrp: &str) -> Result<String> {
    ensure!(
        script.len() == 34 && script[..2] == [0x51, 32] && ["sq", "ssq"].contains(&hrp),
        "unsupported address script or network"
    );
    let mut data = vec![bech32::u5::try_from_u8(1)?];
    data.extend((&script[2..]).to_base32());
    Ok(bech32::encode(hrp, data, Variant::Bech32m)?)
}
pub const MAX_TRANSACTION_INPUTS: usize = 32;

/// Minimum confirmations for a coinbase to be spendable in the next block.
/// Soqucoin applies the maturity tier at the height where the coinbase was mined.
pub fn coinbase_maturity(network: &str, mined_height: u64) -> Option<u64> {
    match network {
        "main" if mined_height > 0 => Some(288),
        "main" => Some(30),
        "stagenet" if mined_height >= 100_000 => Some(288),
        "stagenet" => Some(30),
        "regtest" => Some(60),
        _ => None,
    }
}

/// Parse exact decimal coin amounts without binary floating point or rounding.
pub fn atoms(s: &str) -> Result<u64> {
    ensure!(
        !s.starts_with('-') && !s.contains(['e', 'E', '+']),
        "invalid amount"
    );
    let mut parts = s.split('.');
    let whole = parts.next().context("amount")?;
    let fraction = parts.next().unwrap_or("");
    ensure!(
        parts.next().is_none()
            && !whole.is_empty()
            && whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
            && fraction.len() <= 8,
        "invalid precision"
    );
    let w: u64 = whole.parse()?;
    let f = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>()? * 10u64.pow(8 - fraction.len() as u32)
    };
    match w.checked_mul(100_000_000).and_then(|w| w.checked_add(f)) {
        Some(v) if v <= i64::MAX as u64 => Ok(v),
        _ => bail!("amount overflow"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn amounts() {
        assert_eq!(atoms("12.00000001").unwrap(), 1_200_000_001);
        for s in ["1e8", "-1", "0.000000001", "1.2.3", "18446744073709551615"] {
            assert!(atoms(s).is_err());
        }
    }
    #[test]
    fn address_roundtrip() {
        let pk = [42; 1312];
        let a = address(&pk, "sq").unwrap();
        assert_eq!(&address_script(&a, "sq").unwrap()[2..], &sha256(&pk));
        assert!(address_script(&a, "tsq").is_err());
    }
    #[test]
    fn transaction_roundtrip() {
        let t = Transaction {
            version: 2,
            inputs: vec![Input {
                previous: OutPoint {
                    txid: "12".repeat(32),
                    vout: 1,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![vec![7; 2421], vec![8; 1313]],
            }],
            outputs: vec![Output {
                value: 123,
                script: vec![0x51, 32],
            }],
            lock_time: 0,
        };
        let b = t.encode(true).unwrap();
        assert_eq!(Transaction::decode(&b).unwrap(), t);
        assert_ne!(hash256(&b), hash256(&t.encode(false).unwrap()));
        for n in 0..b.len() {
            assert!(Transaction::decode(&b[..n]).is_err());
        }
    }
}
