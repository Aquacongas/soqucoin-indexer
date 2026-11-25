//! Keys are owned by the browser WASM instance. This crate exposes no server key API.
use anyhow::{ensure, Context, Result};
use fips204::{
    ml_dsa_44::{PrivateKey, PublicKey, KG},
    traits::{KeyGen, SerDes, Signer, Verifier},
};
use soq_core::{address, address_script, hash256, sha256, Output, Transaction};
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

mod recovery;

// Value of new witness-v1 asset outputs in base units (0.01 SOQ).
const ASSET_OUTPUT_ATOMS: u64 = soq_core::protocol::TOKEN_OUTPUT_ATOMS;

pub struct Wallet {
    seed: Zeroizing<[u8; 32]>,
    recovery_entropy: Option<Zeroizing<[u8; 32]>>,
    secret: PrivateKey,
    public: [u8; 1312],
}
impl Wallet {
    pub fn from_seed(seed: [u8; 32]) -> Result<Self> {
        let seed = Zeroizing::new(seed);
        let (public, secret) = KG::keygen_from_seed(&seed);
        let public = public.into_bytes();
        ensure!(
            public[0] != 255,
            "seed produces node-reserved invalid key marker"
        );
        Ok(Self {
            seed,
            recovery_entropy: None,
            secret,
            public,
        })
    }
    pub fn create() -> Result<Self> {
        loop {
            let mut seed = Zeroizing::new([0; 32]);
            getrandom::getrandom(seed.as_mut())
                .map_err(|_| anyhow::anyhow!("secure entropy unavailable"))?;
            if let Ok(wallet) = Self::from_seed(*seed) {
                return Ok(wallet);
            }
        }
    }
    pub fn receive_address(&self, hrp: &str) -> Result<String> {
        address(&self.public, hrp)
    }
    pub fn wif(&self, prefix: u8) -> String {
        let mut v = Zeroizing::new(vec![prefix]);
        v.extend(self.seed.iter());
        v.push(2);
        let check = hash256(&v);
        v.extend(&check[..4]);
        bs58::encode(&*v).into_string()
    }
    pub fn import_wif(wif: &str, prefix: u8) -> Result<Self> {
        ensure!(wif.len() <= 60, "invalid seed WIF length");
        let v = Zeroizing::new(bs58::decode(wif).into_vec()?);
        ensure!(
            v.len() == 38 && v[0] == prefix && v[33] == 2 && hash256(&v[..34])[..4] == v[34..],
            "invalid seed WIF/network/checksum; expanded-key WIF not supported"
        );
        Self::from_seed(v[1..33].try_into()?)
    }
    pub fn sign(&self, tx: &mut Transaction, prevouts: &[Output]) -> Result<()> {
        self.sign_inputs(tx, prevouts)
    }

    fn owns_output(&self, output: &Output) -> bool {
        output.script.len() == 34
            && output.script[..2] == [0x51, 32]
            && output.script[2..] == sha256(&self.public)
    }

    fn sign_input(&self, tx: &mut Transaction, index: usize, prevout: &Output) -> Result<()> {
        ensure!(self.owns_output(prevout), "input not owned by signer");
        let h = tx.sighash(index, prevout)?;
        let mut sig = self
            .secret
            .try_sign_with_seed(&[0; 32], &h, &[])
            .map_err(anyhow::Error::msg)?
            .to_vec();
        sig.push(1);
        let mut pk = vec![0];
        pk.extend(self.public);
        tx.inputs[index].witness = vec![sig, pk];
        Ok(())
    }

    fn sign_inputs(
        &self,
        tx: &mut Transaction,
        prevouts: &[Output],
    ) -> Result<()> {
        ensure!(
            prevouts.len() == tx.inputs.len() && !prevouts.is_empty(),
            "provide every prevout"
        );
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        for (i, p) in tx.inputs.iter().zip(prevouts) {
            ensure!(
                seen.insert((&i.previous.txid, i.previous.vout)),
                "duplicate input"
            );
            ensure!(
                self.owns_output(p) || !i.witness.is_empty(),
                "input not owned by wallet and lacks a counterparty signature"
            );
            ensure!(i.script.is_empty(), "witness scriptSig must be empty");
            total = total.checked_add(p.value).context("input overflow")?;
        }
        let output_total = tx.outputs.iter().try_fold(0u64, |a, o| {
            a.checked_add(o.value).context("output overflow")
        })?;
        ensure!(output_total <= total, "outputs exceed inputs");
        for (i, p) in prevouts.iter().enumerate() {
            if self.owns_output(p) {
                self.sign_input(tx, i, p)?;
            }
        }
        verify(tx, prevouts)
    }
}

pub mod trade;

pub fn verify(tx: &Transaction, prevouts: &[Output]) -> Result<()> {
    ensure!(tx.inputs.len() == prevouts.len(), "prevout count");
    for (i, p) in prevouts.iter().enumerate() {
        verify_input(tx, i, p)?;
    }
    Ok(())
}

fn verify_input(tx: &Transaction, i: usize, p: &Output) -> Result<()> {
    let w = &tx.inputs[i].witness;
    ensure!(
        w.len() == 2
            && w[0].len() == 2421
            && (w[0][2420] == 1 || (i == 0 && w[0][2420] == 0x83))
            && w[1].len() == 1313
            && w[1][0] == 0,
        "invalid witness encoding"
    );
    let pk: [u8; 1312] = w[1][1..].try_into()?;
    ensure!(
        p.script.len() == 34 && p.script[..2] == [0x51, 32] && sha256(&pk) == p.script[2..],
        "ownership mismatch"
    );
    let key = PublicKey::try_from_bytes(pk).map_err(anyhow::Error::msg)?;
    let sig = w[0][..2420].try_into()?;
    ensure!(
        key.verify(&tx.sighash_type(i, p, w[0][2420])?, &sig, &[]),
        "signature verification failed"
    );
    Ok(())
}

#[wasm_bindgen]
pub struct BrowserWallet {
    wallet: Option<Wallet>,
    hrp: String,
    prefix: u8,
}
fn js_error(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}
#[wasm_bindgen]
impl BrowserWallet {
    #[wasm_bindgen(constructor)]
    pub fn new(hrp: String, prefix: u8) -> BrowserWallet {
        Self {
            wallet: None,
            hrp,
            prefix,
        }
    }
    pub fn create(&mut self) -> Result<String, JsValue> {
        self.wallet = Some(Wallet::create().map_err(js_error)?);
        self.address()
    }
    pub fn create_recovery_phrase(&mut self) -> Result<String, JsValue> {
        let mut entropy = Zeroizing::new([0; 32]);
        getrandom::getrandom(entropy.as_mut())
            .map_err(|_| js_error("secure entropy unavailable"))?;
        self.wallet = Some(recovery::derive_wallet(&entropy).map_err(js_error)?);
        Ok(recovery::encode(&entropy))
    }
    pub fn import_recovery_phrase(&mut self, phrase: &str) -> Result<String, JsValue> {
        let entropy = recovery::decode(phrase).map_err(js_error)?;
        self.wallet = Some(recovery::derive_wallet(&entropy).map_err(js_error)?);
        self.address()
    }
    pub fn import(&mut self, wif: &str) -> Result<String, JsValue> {
        self.wallet = Some(Wallet::import_wif(wif, self.prefix).map_err(js_error)?);
        self.address()
    }
    pub fn address(&self) -> Result<String, JsValue> {
        self.wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?
            .receive_address(&self.hrp)
            .map_err(js_error)
    }
    pub fn backup(&self) -> Result<String, JsValue> {
        Ok(self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?
            .wif(self.prefix))
    }
    pub fn backup_recovery_phrase(&self) -> Result<Option<String>, JsValue> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        Ok(wallet
            .recovery_entropy
            .as_ref()
            .map(|entropy| recovery::encode(entropy)))
    }
    pub fn lock(&mut self) {
        self.wallet = None;
    }
    /// The browser constructs and reviews the complete transaction. No network calls.
    pub fn sign_transaction(&self, transaction: &str, prevouts: &str) -> Result<String, JsValue> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        let mut tx: Transaction = serde_json::from_str(transaction).map_err(js_error)?;
        let prev: Vec<Output> = serde_json::from_str(prevouts).map_err(js_error)?;
        wallet.sign(&mut tx, &prev).map_err(js_error)?;
        Ok(hex::encode(tx.encode(true).map_err(js_error)?))
    }
    pub fn destination_script(&self, address: &str) -> Result<String, JsValue> {
        Ok(hex::encode(
            address_script(address, &self.hrp).map_err(js_error)?,
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use soq_core::{Input, OutPoint};

    #[test]
    fn native_max_uses_eligible_inputs_and_keeps_one_output_during_fee_quotes() {
        let mut browser = BrowserWallet::new("sq".to_owned(), 158);
        let owner = Wallet::from_seed([7; 32]).unwrap();
        let recipient = owner.receive_address("sq").unwrap();
        let script = hex::encode(address_script(&recipient, "sq").unwrap());
        browser.wallet = Some(owner);
        let mut coins: Vec<_> = (0u64..38)
            .map(|n| {
                serde_json::json!({
                    "txid":format!("{n:064x}"),"vout":0,"value":(100_000+n).to_string(),
                    "script":script,"coinbase":false,"protected":false,"confirmations":1
                })
            })
            .collect();
        coins[37]["coinbase"] = true.into();
        coins[37]["confirmations"] = 288.into();
        coins[37]["native_spendable"] = true.into();
        coins[36]["coinbase"] = true.into();
        coins[36]["confirmations"] = 287.into();
        coins[36]["native_spendable"] = false.into();
        coins[35]["protected"] = true.into();
        coins[34]["asset"] = "asset-id".into();
        coins[33]["confirmations"] = 0.into();
        let coins = serde_json::to_string(&coins).unwrap();
        let (.., funding) = browser.native_funding(&coins).unwrap();
        assert_eq!(funding.len(), 32);
        assert_eq!(funding[0].0.txid, format!("{:064x}", 37));
        let total: u64 = (2..=32).map(|n| 100_000 + n).sum::<u64>() + 100_037;
        assert_eq!(native_total(&funding).unwrap(), total);
        for fee in [1, 7200] {
            let max = browser
                .build_native_payment(&coins, &recipient, None, fee)
                .unwrap();
            let max: serde_json::Value = serde_json::from_str(&max).unwrap();
            assert_eq!(max["amount_atoms"], (total - fee).to_string());
            assert_eq!(max["change_atoms"], "0");
            assert_eq!(max["transaction"]["outputs"].as_array().unwrap().len(), 1);
        }
        let ordinary = browser
            .build_native_payment(&coins, &recipient, Some(total - 7200), 1)
            .unwrap();
        let ordinary: serde_json::Value = serde_json::from_str(&ordinary).unwrap();
        assert_eq!(
            ordinary["transaction"]["outputs"].as_array().unwrap().len(),
            2
        );
        assert!(browser
            .build_native_payment(&coins, &recipient, Some(total - 7200), 7201)
            .is_err());
    }

    #[test]
    fn seed_wif_and_signing() {
        let w = Wallet::from_seed([7; 32]).unwrap();
        let restored = Wallet::import_wif(&w.wif(158), 158).unwrap();
        assert_eq!(w.public, restored.public);
        assert!(Wallet::import_wif(&w.wif(158), 239).is_err());
        let p = Output {
            value: 10000,
            script: address_script(&w.receive_address("sq").unwrap(), "sq").unwrap(),
        };
        let mut t = Transaction {
            version: 2,
            inputs: vec![Input {
                previous: OutPoint {
                    txid: "ab".repeat(32),
                    vout: 0,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            }],
            outputs: vec![Output {
                value: 9000,
                script: p.script.clone(),
            }],
            lock_time: 0,
        };
        w.sign(&mut t, std::slice::from_ref(&p)).unwrap();
        verify(&t, std::slice::from_ref(&p)).unwrap();
        t.outputs[0].value -= 1;
        assert!(verify(&t, &[p]).is_err());
    }

}

#[derive(serde::Deserialize)]
struct Spendable {
    txid: String,
    vout: u32,
    value: String,
    script: String,
    coinbase: bool,
    protected: bool,
    #[serde(default)]
    confirmations: u64,
    #[serde(default)]
    native_spendable: Option<bool>,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    quantity: Option<String>,
}
#[wasm_bindgen]
impl BrowserWallet {
    pub fn native_send_available(&self, coins: &str) -> Result<String, JsValue> {
        self.native_funding(coins)
            .and_then(|(_, inputs)| native_total(&inputs))
            .map(|amount| amount.to_string())
            .map_err(js_error)
    }

    pub fn prepare_payment(
        &self,
        coins: &str,
        recipient: &str,
        amount_soq: &str,
        network_fee_soq: &str,
    ) -> Result<String, JsValue> {
        let amount = soq_core::atoms(amount_soq).map_err(js_error)?;
        let fee = soq_core::atoms(network_fee_soq).map_err(js_error)?;
        self.build_native_payment(coins, recipient, Some(amount), fee)
            .map_err(js_error)
    }

    pub fn prepare_max_payment(
        &self,
        coins: &str,
        recipient: &str,
        network_fee_soq: &str,
    ) -> Result<String, JsValue> {
        let fee = soq_core::atoms(network_fee_soq).map_err(js_error)?;
        self.build_native_payment(coins, recipient, None, fee)
            .map_err(js_error)
    }
}

type NativeFunding = Vec<(soq_core::OutPoint, Output)>;

fn native_total(inputs: &NativeFunding) -> Result<u64> {
    inputs.iter().try_fold(0_u64, |sum, (_, out)| {
        sum.checked_add(out.value).context("input overflow")
    })
}

impl BrowserWallet {
    fn native_funding(&self, coins: &str) -> Result<(Vec<u8>, NativeFunding)> {
        let wallet = self.wallet.as_ref().context("wallet locked")?;
        let own = address_script(&wallet.receive_address(&self.hrp)?, &self.hrp)?;
        let list: Vec<Spendable> = serde_json::from_str(coins)?;
        let mut seen = std::collections::HashSet::new();
        let mut inputs = Vec::new();
        for coin in list.into_iter().filter(|coin| {
            coin.confirmations >= 1
                && !coin.protected
                && coin.asset.is_none()
                && coin.native_spendable.unwrap_or(!coin.coinbase)
        }) {
            ensure!(hex::decode(coin.script)? == own, "UTXO ownership mismatch");
            ensure!(
                seen.insert((coin.txid.clone(), coin.vout)),
                "duplicate input"
            );
            let value = coin.value.parse()?;
            if value > 0 {
                inputs.push((
                    soq_core::OutPoint {
                        txid: coin.txid,
                        vout: coin.vout,
                    },
                    Output {
                        value,
                        script: own.clone(),
                    },
                ));
            }
        }
        inputs.sort_by_key(|(_, out)| std::cmp::Reverse(out.value));
        inputs.truncate(soq_core::MAX_TRANSACTION_INPUTS);
        Ok((own, inputs))
    }

    fn build_native_payment(
        &self,
        coins: &str,
        recipient: &str,
        amount: Option<u64>,
        fee: u64,
    ) -> Result<String> {
        ensure!(fee > 0, "network fee must be positive");
        let (own, eligible) = self.native_funding(coins)?;
        let amount = match amount {
            Some(amount) => amount,
            None => native_total(&eligible)?
                .checked_sub(fee)
                .context("insufficient spendable SOQ for network fees")?,
        };
        ensure!(amount > 0, "send amount must be positive");
        let required = amount.checked_add(fee).context("amount overflow")?;
        let mut total = 0u64;
        let mut inputs = Vec::new();
        let mut prevouts = Vec::new();
        for (previous, output) in eligible {
            total = total.checked_add(output.value).context("input overflow")?;
            inputs.push(soq_core::Input {
                previous,
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            });
            prevouts.push(output);
            if total >= required {
                break;
            }
        }
        ensure!(
            total >= required,
            "insufficient spendable SOQ for the amount and network fee"
        );
        let mut outputs = vec![Output {
            value: amount,
            script: address_script(recipient, &self.hrp)?,
        }];
        if total > required {
            outputs.push(Output {
                value: total - required,
                script: own,
            });
        }
        let tx = Transaction {
            version: 2,
            inputs,
            outputs,
            lock_time: 0,
        };
        Ok(serde_json::json!({"transaction":tx,"prevouts":prevouts,"recipient":recipient,"amount_atoms":amount.to_string(),"network_fee_atoms":fee.to_string(),"platform_fee_atoms":"0","platform_recipient":soq_core::fees::FEE_ADDRESS,"input_total_atoms":total.to_string(),"change_atoms":(total-required).to_string()}).to_string())
    }
}

#[wasm_bindgen]
impl BrowserWallet {
    pub fn receive_qr(&self) -> Result<String, JsValue> {
        let address = self.address()?;
        let code = qrcode::QrCode::new(address.as_bytes()).map_err(js_error)?;
        Ok(code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(192, 192)
            .build())
    }
    pub fn prepare_operation(
        &self,
        coins: &str,
        operation: &str,
        network_fee_soq: &str,
        discount: u8,
    ) -> Result<String, JsValue> {
        self.build_operation(
            coins,
            operation,
            network_fee_soq,
            false,
            discount,
        )
    }

    /// Size a no-change mint using the supplied inputs, even when they cannot fund it.
    pub fn operation_fee_template(
        &self,
        coins: &str,
        operation: &str,
        discount: u8,
    ) -> Result<String, JsValue> {
        self.build_operation(coins, operation, "0", true, discount)
    }
}

impl BrowserWallet {
    fn build_operation(
        &self,
        coins: &str,
        operation: &str,
        network_fee_soq: &str,
        template_only: bool,
        discount: u8,
    ) -> Result<String, JsValue> {
        let op: soq_core::protocol::Operation =
            serde_json::from_str(operation).map_err(js_error)?;
        let fatcat = matches!(op, soq_core::protocol::Operation::FatcatRequest { .. });
        let discount = if fatcat { 0 } else { discount };
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        let fee = soq_core::atoms(network_fee_soq).map_err(js_error)?;
        if fee == 0 && !template_only {
            return Err(js_error("network fee must be positive"));
        }
        let own = address_script(
            &wallet.receive_address(&self.hrp).map_err(js_error)?,
            &self.hrp,
        )
        .map_err(js_error)?;
        let anchor = if matches!(op, soq_core::protocol::Operation::Listing { .. }) {
            0
        } else {
            ASSET_OUTPUT_ATOMS
        };
        let mut outputs = Vec::new();
        if anchor > 0 {
            outputs.push(Output { value: anchor, script: own.clone() });
        }
        outputs.push(Output {
            value: 0,
            script: op.script().map_err(js_error)?,
        });
        let mut platform = 0;
        if let Some(kind) = op.platform_kind() {
            let output = soq_core::fees::payment(
                soq_core::fees::discounted_fee(kind.fee(), discount).map_err(js_error)?,
            )
            .map_err(js_error)?;
            platform = output.value;
            outputs.push(output);
        }
        let required = anchor
            .checked_add(platform)
            .and_then(|n| n.checked_add(fee))
            .ok_or_else(|| js_error("amount overflow"))?;
        let list: Vec<Spendable> = serde_json::from_str(coins).map_err(js_error)?;
        let mut total = 0u64;
        let mut inputs = Vec::new();
        let mut prevouts = Vec::new();
        for c in list.into_iter()
            .filter(|c| !c.coinbase && !c.protected && (!fatcat || c.confirmations >= 6))
            .take(32)
        {
            let script = hex::decode(c.script).map_err(js_error)?;
            if script != own {
                return Err(js_error("UTXO ownership mismatch"));
            }
            let value: u64 = c.value.parse().map_err(js_error)?;
            total = total
                .checked_add(value)
                .ok_or_else(|| js_error("input overflow"))?;
            inputs.push(soq_core::Input {
                previous: soq_core::OutPoint {
                    txid: c.txid,
                    vout: c.vout,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            });
            prevouts.push(Output { value, script });
            if !template_only && total >= required {
                break;
            }
        }
        if inputs.is_empty() || (!template_only && total < required)
        {
            return Err(js_error("insufficient eligible confirmed SOQ"));
        }
        if !template_only && total > required {
            outputs.push(Output {
                value: total - required,
                script: own,
            });
        }
        let tx = Transaction {
            version: 2,
            inputs,
            outputs,
            lock_time: 0,
        };
        if template_only {
            let raw = self.fee_template(&serde_json::json!({"transaction":tx}).to_string())?;
            return Ok(serde_json::json!({
                "raw": raw,
                "input_total_atoms": total.to_string(),
                "platform_fee_atoms": platform.to_string(),
                "asset_output_atoms": anchor.to_string(),
            })
            .to_string());
        }
        Ok(serde_json::json!({"transaction":tx,"prevouts":prevouts,"operation":op,"platform_fee_atoms":platform.to_string(),"original_platform_fee_atoms":op.platform_kind().map_or(0, |kind| kind.fee()).to_string(),"discount_percent":discount,"platform_recipient":soq_core::fees::FEE_ADDRESS,"network_fee_atoms":fee.to_string(),"asset_output_atoms":anchor.to_string(),"total_atoms":required.to_string(),"change_atoms":(total-required).to_string()}).to_string())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EncryptedBackup {
    version: u8,
    hrp: String,
    prefix: u8,
    salt: String,
    nonce: String,
    ciphertext: String,
}
fn storage_key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(
        password.len() >= 12 && password.len() <= 1024,
        "password must have 12 to 1024 bytes"
    );
    let mut key = Zeroizing::new([0; 32]);
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| anyhow::anyhow!("password derivation failed"))?;
    Ok(key)
}
impl Wallet {
    pub fn encrypt(&self, password: &str, hrp: &str, prefix: u8) -> Result<String> {
        use aes_gcm::{
            aead::{Aead, Payload},
            Aes256Gcm, KeyInit,
        };
        let mut salt = [0; 16];
        let mut nonce = [0; 12];
        getrandom::getrandom(&mut salt).map_err(|_| anyhow::anyhow!("entropy unavailable"))?;
        getrandom::getrandom(&mut nonce).map_err(|_| anyhow::anyhow!("entropy unavailable"))?;
        let key = storage_key(password, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .map_err(|_| anyhow::anyhow!("encryption key"))?;
        let (version, secret) = match &self.recovery_entropy {
            Some(entropy) => (2, entropy.as_ref()),
            None => (1, self.seed.as_ref()),
        };
        let aad = format!("SOQ.MARKET/wallet/v{version}/{hrp}/{prefix}");
        let ciphertext = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: secret,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        Ok(serde_json::to_string(&EncryptedBackup {
            version,
            hrp: hrp.into(),
            prefix,
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        })?)
    }
    pub fn decrypt(data: &str, password: &str, hrp: &str, prefix: u8) -> Result<Self> {
        use aes_gcm::{
            aead::{Aead, Payload},
            Aes256Gcm, KeyInit,
        };
        ensure!(data.len() <= 1024, "encrypted backup too large");
        let b: EncryptedBackup = serde_json::from_str(data)?;
        ensure!(
            matches!(b.version, 1 | 2) && b.hrp == hrp && b.prefix == prefix,
            "backup network/version mismatch"
        );
        let salt = hex::decode(b.salt)?;
        let nonce: [u8; 12] = hex::decode(b.nonce)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("nonce size"))?;
        let ciphertext = hex::decode(b.ciphertext)?;
        ensure!(
            salt.len() == 16 && ciphertext.len() == 48,
            "invalid backup size"
        );
        let key = storage_key(password, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .map_err(|_| anyhow::anyhow!("encryption key"))?;
        let aad = format!("SOQ.MARKET/wallet/v{}/{hrp}/{prefix}", b.version);
        let seed = Zeroizing::new(
            cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &ciphertext,
                        aad: aad.as_bytes(),
                    },
                )
                .map_err(|_| anyhow::anyhow!("incorrect password or damaged backup"))?,
        );
        if b.version == 2 {
            recovery::derive_wallet(seed.as_slice().try_into()?)
        } else {
            Self::from_seed(seed.as_slice().try_into()?)
        }
    }
}
#[wasm_bindgen]
impl BrowserWallet {
    pub fn encrypted_backup(&self, password: &str) -> Result<String, JsValue> {
        self.wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?
            .encrypt(password, &self.hrp, self.prefix)
            .map_err(js_error)
    }
    pub fn unlock_backup(&mut self, data: &str, password: &str) -> Result<String, JsValue> {
        self.wallet =
            Some(Wallet::decrypt(data, password, &self.hrp, self.prefix).map_err(js_error)?);
        self.address()
    }
    pub fn transaction_id(&self, raw: &str) -> Result<String, JsValue> {
        Transaction::decode(&hex::decode(raw).map_err(js_error)?)
            .map_err(js_error)?
            .txid()
            .map_err(js_error)
    }
    pub fn creation_name_key(&self, name: &str) -> Result<String, JsValue> {
        soq_core::protocol::name_key(name)
            .map(hex::encode)
            .map_err(js_error)
    }

    pub fn operation_json(&self, form: &str) -> Result<String, JsValue> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Form {
            kind: String,
            metadata: String,
            reference: String,
            maximum: String,
            limit: String,
            ticker: String,
            #[serde(default)]
            name: String,
        }
        let form: Form = serde_json::from_str(form).map_err(js_error)?;
        let kind = form.kind.as_str();
        let metadata = form.metadata.as_str();
        let reference = form.reference.as_str();
        let maximum = form.maximum.as_str();
        let limit = form.limit.as_str();
        let ticker = form.ticker.as_str();
        use soq_core::{hash_bytes, protocol::Operation};
        let op = match kind {
            "collection" => Operation::Collection {
                metadata: hash_bytes(metadata).map_err(js_error)?,
                maximum: maximum.parse().map_err(js_error)?,
                name_key: Some(soq_core::protocol::name_key(&form.name).map_err(js_error)?),
            },
            "nft" => {
                if reference.is_empty() {
                    Operation::Nft {
                        metadata: hash_bytes(metadata).map_err(js_error)?,
                        destination: 0,
                    }
                } else {
                    Operation::CollectionNft {
                        collection: hash_bytes(reference).map_err(js_error)?,
                        metadata: hash_bytes(metadata).map_err(js_error)?,
                        destination: 0,
                    }
                }
            }
            "deploy" => {
                if ticker.is_empty() || ticker.len() > 8 {
                    return Err(js_error("ticker must have 1 to 8 characters"));
                }
                let mut symbol = [0; 8];
                symbol[..ticker.len()].copy_from_slice(ticker.as_bytes());
                Operation::Deploy {
                    metadata: hash_bytes(metadata).map_err(js_error)?,
                    cap: maximum.parse().map_err(js_error)?,
                    limit: limit.parse().map_err(js_error)?,
                    ticker: symbol,
                    name_key: Some(soq_core::protocol::name_key(&form.name).map_err(js_error)?),
                }
            }
            "mint" => Operation::Mint {
                token: hash_bytes(reference).map_err(js_error)?,
                amount: maximum.parse().map_err(js_error)?,
                destination: 0,
            },
            "map" => Operation::MapRequest { destination: 0 },
            "soqumap" => Operation::SoquMapRequest { destination: 0 },
            "fatcat" => Operation::FatcatRequest { destination: 0 },
            _ => return Err(js_error("unknown operation")),
        };
        serde_json::to_string(&op).map_err(js_error)
    }
}

#[wasm_bindgen]
impl BrowserWallet {
    pub fn prepare_transfer(
        &self,
        coins: &str,
        asset: &str,
        quantity: &str,
        recipient: &str,
        network_fee_soq: &str,
        discount: u8,
    ) -> Result<String, JsValue> {
        self.prepare_asset_transfer(
            coins,
            asset,
            quantity,
            recipient,
            network_fee_soq,
            false,
            discount,
        )
    }
    pub fn prepare_cancellation(
        &self,
        coins: &str,
        asset: &str,
        quantity: &str,
        network_fee_soq: &str,
    ) -> Result<String, JsValue> {
        self.prepare_asset_transfer(
            coins,
            asset,
            quantity,
            &self.address()?,
            network_fee_soq,
            true,
            0,
        )
    }
}
impl BrowserWallet {
    fn prepare_asset_transfer(
        &self,
        coins: &str,
        asset: &str,
        quantity: &str,
        recipient: &str,
        network_fee_soq: &str,
        cancellation: bool,
        discount: u8,
    ) -> Result<String, JsValue> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        let asset_hash = soq_core::hash_bytes(asset).map_err(js_error)?;
        let quantity: u64 = quantity.parse().map_err(js_error)?;
        if quantity == 0 {
            return Err(js_error("quantity must be positive"));
        }
        let fee = soq_core::atoms(network_fee_soq).map_err(js_error)?;
        let own = address_script(
            &wallet.receive_address(&self.hrp).map_err(js_error)?,
            &self.hrp,
        )
        .map_err(js_error)?;
        let recipient = address_script(recipient, &self.hrp).map_err(js_error)?;
        let list: Vec<Spendable> = serde_json::from_str(coins).map_err(js_error)?;
        let mut selected = Vec::new();
        let mut token_total = 0u64;
        for c in list
            .iter()
            .filter(|c| !c.coinbase && c.asset.as_deref() == Some(asset))
            .take(24)
        {
            let q: u64 = c
                .quantity
                .as_ref()
                .ok_or_else(|| js_error("missing asset quantity"))?
                .parse()
                .map_err(js_error)?;
            token_total = token_total
                .checked_add(q)
                .ok_or_else(|| js_error("quantity overflow"))?;
            selected.push(c);
            if token_total >= quantity {
                break;
            }
        }
        if token_total < quantity {
            return Err(js_error("insufficient asset balance"));
        }
        let mut allocations = vec![(0, quantity)];
        let mut outputs = vec![Output {
            value: ASSET_OUTPUT_ATOMS,
            script: recipient,
        }];
        if token_total > quantity {
            allocations.push((1, token_total - quantity));
            outputs.push(Output {
                value: ASSET_OUTPUT_ATOMS,
                script: own.clone(),
            });
        }
        if cancellation && (selected.len() != 1 || token_total != quantity) {
            return Err(js_error(
                "cancellation requires returning exactly one complete asset lot",
            ));
        }
        let original_platform = if cancellation {
            0
        } else {
            soq_core::fees::TRANSFER_FEE
        };
        let platform =
            soq_core::fees::discounted_fee(original_platform, discount).map_err(js_error)?;
        let required = ((outputs.len() as u64) * ASSET_OUTPUT_ATOMS)
            .checked_add(fee)
            .and_then(|value| value.checked_add(platform))
            .ok_or_else(|| js_error("transfer amount overflow"))?;
        let mut total = selected.iter().try_fold(0u64, |s, c| {
            let value: u64 = c.value.parse().map_err(js_error)?;
            s.checked_add(value)
                .ok_or_else(|| js_error("input overflow"))
        })?;
        for c in list.iter().filter(|c| !c.coinbase && !c.protected).take(8) {
            if total >= required {
                break;
            }
            total = total
                .checked_add(c.value.parse().map_err(js_error)?)
                .ok_or_else(|| js_error("input overflow"))?;
            selected.push(c);
        }
        if total < required {
            return Err(js_error(
                "insufficient SOQ for transfer outputs and network fee",
            ));
        }
        let op = if cancellation {
            soq_core::protocol::Operation::Cancel {
                asset: asset_hash,
                quantity,
            }
        } else {
            soq_core::protocol::Operation::Transfer {
                asset: asset_hash,
                allocations,
            }
        };
        outputs.push(Output {
            value: 0,
            script: op.script().map_err(js_error)?,
        });
        if platform > 0 {
            outputs.push(soq_core::fees::payment(platform).map_err(js_error)?);
        }
        if total > required {
            outputs.push(Output {
                value: total - required,
                script: own.clone(),
            });
        }
        let mut inputs = Vec::new();
        let mut prevouts = Vec::new();
        for c in selected {
            let script = hex::decode(&c.script).map_err(js_error)?;
            if script != own {
                return Err(js_error("UTXO ownership mismatch"));
            }
            prevouts.push(Output {
                value: c.value.parse().map_err(js_error)?,
                script,
            });
            inputs.push(soq_core::Input {
                previous: soq_core::OutPoint {
                    txid: c.txid.clone(),
                    vout: c.vout,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            });
        }
        let tx = Transaction {
            version: 2,
            inputs,
            outputs,
            lock_time: 0,
        };
        Ok(serde_json::json!({"transaction":tx,"prevouts":prevouts,"operation":op,"change_atoms":(total-required).to_string(),"network_fee_atoms":fee.to_string(),"platform_fee_atoms":platform.to_string(),"original_platform_fee_atoms":original_platform.to_string(),"discount_percent":discount,"platform_recipient":soq_core::fees::FEE_ADDRESS}).to_string())
    }
}

#[wasm_bindgen]
impl BrowserWallet {
    /// Encode the exact signed structure without generating a signature or exposing keys.
    pub fn fee_template(&self, review: &str) -> Result<String, JsValue> {
        self.address()?;
        let v: serde_json::Value = serde_json::from_str(review).map_err(js_error)?;
        let mut tx: Transaction =
            serde_json::from_value(v["transaction"].clone()).map_err(js_error)?;
        for input in &mut tx.inputs {
            if input.witness.is_empty() {
                input.witness = vec![vec![0; 2421], vec![0; 1313]];
            }
        }
        Ok(hex::encode(tx.encode(true).map_err(js_error)?))
    }
    pub fn review_summary(&self, review: &str) -> Result<String, JsValue> {
        let v: serde_json::Value = serde_json::from_str(review).map_err(js_error)?;
        let tx: Transaction = serde_json::from_value(v["transaction"].clone()).map_err(js_error)?;
        let outputs=tx.outputs.iter().enumerate().map(|(index,o)|serde_json::json!({"index":index,"recipient":soq_core::script_address(&o.script,&self.hrp).unwrap_or_else(|_|"On-chain protocol data".into()),"soq":format!("{}.{:08}",o.value/100_000_000,o.value%100_000_000),"atoms":o.value.to_string()})).collect::<Vec<_>>();
        Ok(serde_json::json!({"operation":v["operation"],"outputs":outputs,"platform_fee_atoms":v["platform_fee_atoms"],"platform_recipient":soq_core::fees::FEE_ADDRESS,"network_fee_atoms":v["network_fee_atoms"]}).to_string())
    }
}
