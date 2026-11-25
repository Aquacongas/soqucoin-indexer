//! Single-UTXO full-lot offers. Seller signature commits to payment at output zero.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub transaction: Transaction,
    pub prevout: Output,
    pub asset: String,
    pub quantity: String,
    pub price_atoms: String,
    #[serde(default)]
    pub dex: bool,
    #[serde(default)]
    pub seller_discount_percent: u8,
}
pub fn verify_offer(offer: &Offer) -> Result<()> {
    let tx = &offer.transaction;
    ensure!(
        tx.version == 2
            && tx.lock_time == 0
            && tx.inputs.len() == 1
            && tx.outputs.len() == 1
            && tx.inputs[0].script.is_empty()
            && tx.inputs[0].sequence == u32::MAX,
        "invalid offer template"
    );
    ensure!(
        tx.inputs[0].witness.first().and_then(|s| s.last()) == Some(&0x83),
        "offer requires SINGLE|ANYONECANPAY signature"
    );
    let price: u64 = offer.price_atoms.parse()?;
    let quantity: u64 = offer.quantity.parse()?;
    let commission = if offer.dex {
        soq_core::fees::discounted_fee(
            soq_core::fees::dex_commission(price)?,
            offer.seller_discount_percent,
        )?
    } else {
        ensure!(
            offer.seller_discount_percent == 0,
            "this sale has no seller platform fee"
        );
        0
    };
    ensure!(
        price > 0
            && quantity > 0
            && tx.outputs[0].script == offer.prevout.script
            && tx.outputs[0].value
                == offer
                    .prevout
                    .value
                    .checked_add(price - commission)
                    .context("offer price overflow")?,
        "invalid offer payment"
    );
    soq_core::hash_bytes(&offer.asset)?;
    verify(tx, std::slice::from_ref(&offer.prevout))
}
#[wasm_bindgen]
impl BrowserWallet {
    pub fn create_offer(
        &self,
        note: &str,
        price_soq: &str,
        dex: bool,
        seller_discount_percent: u8,
    ) -> Result<String, JsValue> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        let c: Spendable = serde_json::from_str(note).map_err(js_error)?;
        if c.coinbase || !c.protected {
            return Err(js_error("select an eligible asset UTXO"));
        }
        let asset = c
            .asset
            .ok_or_else(|| js_error("map not revealed or asset missing"))?;
        let quantity = c
            .quantity
            .ok_or_else(|| js_error("asset quantity missing"))?;
        let price = soq_core::atoms(price_soq).map_err(js_error)?;
        let commission = if dex {
            soq_core::fees::dex_commission(price)
                .and_then(|fee| soq_core::fees::discounted_fee(fee, seller_discount_percent))
                .map_err(js_error)?
        } else {
            0
        };
        let value: u64 = c.value.parse().map_err(js_error)?;
        let script = hex::decode(c.script).map_err(js_error)?;
        if script.len() != 34 || script[..2] != [0x51, 32] || script[2..] != sha256(&wallet.public)
        {
            return Err(js_error("asset is not owned by wallet"));
        }
        let prevout = Output {
            value,
            script: script.clone(),
        };
        let mut tx = Transaction {
            version: 2,
            inputs: vec![soq_core::Input {
                previous: soq_core::OutPoint {
                    txid: c.txid,
                    vout: c.vout,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            }],
            outputs: vec![Output {
                value: value
                    .checked_add(price - commission)
                    .ok_or_else(|| js_error("price overflow"))?,
                script,
            }],
            lock_time: 0,
        };
        let hash = tx.sighash_type(0, &prevout, 0x83).map_err(js_error)?;
        let mut sig = wallet
            .secret
            .try_sign_with_seed(&[0; 32], &hash, &[])
            .map_err(js_error)?
            .to_vec();
        sig.push(0x83);
        let mut public = vec![0];
        public.extend(wallet.public);
        tx.inputs[0].witness = vec![sig, public];
        let offer = Offer {
            transaction: tx,
            prevout,
            asset,
            quantity,
            price_atoms: price.to_string(),
            dex,
            seller_discount_percent,
        };
        verify_offer(&offer).map_err(js_error)?;
        serde_json::to_string(&offer).map_err(js_error)
    }
    pub fn listing_operation(&self, offer: &str) -> Result<String, JsValue> {
        Ok(
            serde_json::to_string(&soq_core::protocol::Operation::Listing {
                offer: sha256(offer.as_bytes()),
            })
            .map_err(js_error)?,
        )
    }
    pub fn prepare_purchase(
        &self,
        offer_json: &str,
        coins: &str,
        network_fee_soq: &str,
        buyer_discount: u8,
    ) -> Result<String, JsValue> {
        let offer: Offer = serde_json::from_str(offer_json).map_err(js_error)?;
        verify_offer(&offer).map_err(js_error)?;
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        let own = address_script(
            &wallet.receive_address(&self.hrp).map_err(js_error)?,
            &self.hrp,
        )
        .map_err(js_error)?;
        if own == offer.prevout.script {
            return Err(js_error("cannot purchase your own listing"));
        }
        let fee = soq_core::atoms(network_fee_soq).map_err(js_error)?;
        let price: u64 = offer.price_atoms.parse().map_err(js_error)?;
        let original_seller = if offer.dex {
            soq_core::fees::dex_commission(price).map_err(js_error)?
        } else {
            0
        };
        let original_buyer = if offer.dex {
            original_seller
        } else {
            soq_core::fees::PURCHASE_FEE
        };
        let seller_commission =
            soq_core::fees::discounted_fee(original_seller, offer.seller_discount_percent)
                .map_err(js_error)?;
        let buyer_commission =
            soq_core::fees::discounted_fee(original_buyer, buyer_discount).map_err(js_error)?;
        let platform = buyer_commission + seller_commission;
        let required = price
            .checked_add(ASSET_OUTPUT_ATOMS)
            .and_then(|n| n.checked_add(fee))
            .and_then(|n| n.checked_add(buyer_commission))
            .ok_or_else(|| js_error("purchase amount overflow"))?;
        let mut tx = offer.transaction.clone();
        let mut prevouts = vec![offer.prevout];
        let mut total = 0u64;
        let coins: Vec<Spendable> = serde_json::from_str(coins).map_err(js_error)?;
        for c in coins
            .into_iter()
            .filter(|c| !c.coinbase && !c.protected)
            .take(31)
        {
            let script = hex::decode(c.script).map_err(js_error)?;
            if script != own {
                return Err(js_error("funding input not owned"));
            }
            let value: u64 = c.value.parse().map_err(js_error)?;
            total = total
                .checked_add(value)
                .ok_or_else(|| js_error("funding overflow"))?;
            tx.inputs.push(soq_core::Input {
                previous: soq_core::OutPoint {
                    txid: c.txid,
                    vout: c.vout,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            });
            prevouts.push(Output { value, script });
            if total >= required {
                break;
            }
        }
        if total < required {
            return Err(js_error("insufficient eligible SOQ"));
        }
        tx.outputs.push(Output {
            value: ASSET_OUTPUT_ATOMS,
            script: own.clone(),
        });
        let op = soq_core::protocol::Operation::Settle {
            asset: soq_core::hash_bytes(&offer.asset).map_err(js_error)?,
            quantity: offer.quantity.parse().map_err(js_error)?,
            price,
        };
        tx.outputs.push(Output {
            value: 0,
            script: op.script().map_err(js_error)?,
        });
        tx.outputs
            .push(soq_core::fees::payment(platform).map_err(js_error)?);
        if total > required {
            tx.outputs.push(Output {
                value: total - required,
                script: own,
            });
        }
        Ok(serde_json::json!({"transaction":tx,"prevouts":prevouts,"operation":op,"price_atoms":offer.price_atoms,"quantity":offer.quantity,"change_atoms":(total-required).to_string(),"network_fee_atoms":fee.to_string(),"platform_fee_atoms":buyer_commission.to_string(),"original_platform_fee_atoms":original_buyer.to_string(),"discount_percent":buyer_discount,"original_seller_fee_atoms":original_seller.to_string(),"seller_discount_percent":offer.seller_discount_percent,"seller_commission_atoms":seller_commission.to_string(),"platform_total_atoms":platform.to_string(),"platform_recipient":soq_core::fees::FEE_ADDRESS}).to_string())
    }
    pub fn sign_review(&self, review: &str) -> Result<String, JsValue> {
        #[derive(Deserialize)]
        struct Review {
            transaction: Transaction,
            prevouts: Vec<Output>,
        }
        let mut r: Review = serde_json::from_str(review).map_err(js_error)?;
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| js_error("wallet locked"))?;
        wallet.sign(&mut r.transaction, &r.prevouts).map_err(js_error)?;
        Ok(hex::encode(r.transaction.encode(true).map_err(js_error)?))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_operation_json_rejects_obsolete_precision_and_fractional_quantities() {
        use soq_core::protocol::Operation;
        let deploy = Operation::Deploy {
            metadata: [1; 32],
            cap: 100,
            limit: 10,
            ticker: *b"TEST\0\0\0\0",
            name_key: None,
        };
        let mut request = serde_json::to_value(deploy).unwrap();
        request["Deploy"]["decimals"] = serde_json::json!(0);
        assert!(serde_json::from_value::<Operation>(request).is_err());
        assert!(serde_json::from_value::<Operation>(serde_json::json!({
            "Mint":{"token":vec![0;32],"amount":1.5,"destination":0}
        }))
        .is_err());
    }

    #[test]
    fn token_split_preserves_units_and_uses_soq_for_the_second_output() {
        let mut seller = BrowserWallet::new("sq".into(), 158);
        seller.wallet = Some(Wallet::from_seed([7; 32]).unwrap());
        let address = seller.address().unwrap();
        let own = seller.destination_script(&address).unwrap();
        let receiver = Wallet::from_seed([8; 32])
            .unwrap()
            .receive_address("sq")
            .unwrap();
        let asset = "ab".repeat(32);
        let coins = serde_json::json!([
            {"txid":"ef".repeat(32),"vout":0,"value":"1000000","script":own,
             "coinbase":false,"protected":true,"asset":asset,"quantity":"10"},
            {"txid":"cd".repeat(32),"vout":1,"value":"200000000","script":own,
             "coinbase":false,"protected":false}
        ]);
        let raw = seller
            .prepare_transfer(&coins.to_string(), &asset, "1", &receiver, "0.0012", 0)
            .unwrap();
        let review: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let transaction: Transaction =
            serde_json::from_value(review["transaction"].clone()).unwrap();
        let op = soq_core::protocol::operation(&transaction.outputs).unwrap();
        assert_eq!(
            op,
            soq_core::protocol::Operation::Transfer {
                asset: soq_core::hash_bytes(&asset).unwrap(),
                allocations: vec![(0, 1), (1, 9)]
            }
        );
        assert_eq!(transaction.outputs[0].value, 1_000_000);
        assert_eq!(transaction.outputs[1].value, 1_000_000);
        assert_eq!(
            transaction.outputs[0].script,
            address_script(&receiver, "sq").unwrap()
        );
        assert_eq!(transaction.outputs[1].script, hex::decode(own).unwrap());
        assert_eq!(transaction.inputs.len(), 2);
        assert_eq!(
            201_000_000 - transaction.outputs.iter().map(|out| out.value).sum::<u64>(),
            120_000
        );
    }

    #[test]
    fn consolidated_sell_lot_returns_token_and_soq_change() {
        let mut seller = BrowserWallet::new("sq".into(), 158);
        seller.wallet = Some(Wallet::from_seed([7; 32]).unwrap());
        let address = seller.address().unwrap();
        let script = seller.destination_script(&address).unwrap();
        let asset = "ab".repeat(32);
        let coins = serde_json::json!([
            {"txid":"ef".repeat(32),"vout":0,"value":"1000000","script":script,
             "coinbase":false,"protected":true,"asset":asset,"quantity":"6"},
            {"txid":"fe".repeat(32),"vout":0,"value":"1000000","script":script,
             "coinbase":false,"protected":true,"asset":asset,"quantity":"5"},
            {"txid":"cd".repeat(32),"vout":1,"value":"200000000","script":script,
             "coinbase":false,"protected":false}
        ]);
        let raw = seller
            .prepare_transfer(&coins.to_string(), &asset, "8", &address, "0.0012", 0)
            .unwrap();
        let review: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let tx: Transaction = serde_json::from_value(review["transaction"].clone()).unwrap();
        assert_eq!(tx.inputs.len(), 3);
        assert_eq!(
            soq_core::protocol::operation(&tx.outputs).unwrap(),
            soq_core::protocol::Operation::Transfer {
                asset: soq_core::hash_bytes(&asset).unwrap(),
                allocations: vec![(0, 8), (1, 3)],
            }
        );
        let own = hex::decode(script).unwrap();
        for output in &tx.outputs[..2] {
            assert_eq!(output.value, 1_000_000);
            assert_eq!(output.script, own);
        }
        let change = tx.outputs.last().unwrap();
        assert_eq!(change.script, own);
        assert_eq!(change.value, 99_880_000);
        assert_eq!(
            202_000_000 - tx.outputs.iter().map(|output| output.value).sum::<u64>(),
            120_000
        );
    }

    #[test]
    fn browser_wallet_signs_a_complete_soq_token_settlement() {
        let mut seller = BrowserWallet::new("sq".into(), 158);
        seller.wallet = Some(Wallet::from_seed([7; 32]).unwrap());
        let mut buyer = BrowserWallet::new("sq".into(), 158);
        buyer.wallet = Some(Wallet::from_seed([8; 32]).unwrap());
        let seller_script = seller
            .destination_script(&seller.address().unwrap())
            .unwrap();
        let buyer_script = buyer.destination_script(&buyer.address().unwrap()).unwrap();
        let asset = "ab".repeat(32);
        let lot = serde_json::json!({
            "txid":"ef".repeat(32), "vout":0, "value":"1000000", "script":seller_script,
            "coinbase":false, "protected":true, "asset":asset, "quantity":"1"
        });
        let offer = seller
            .create_offer(&lot.to_string(), "10000", true, 0)
            .unwrap();
        let signed_offer: Offer = serde_json::from_str(&offer).unwrap();
        verify_offer(&signed_offer).unwrap();
        assert!(offer.len() <= 20_000);
        let funding = serde_json::json!([{
            "txid":"cd".repeat(32), "vout":0, "value":"1200000000000", "script":buyer_script,
            "coinbase":false, "protected":false
        }]);
        let reviewed = buyer
            .prepare_purchase(&offer, &funding.to_string(), "0.0012", 0)
            .unwrap();
        let signed = buyer.sign_review(&reviewed).unwrap();
        let transaction = Transaction::decode(&hex::decode(signed).unwrap()).unwrap();
        let review: serde_json::Value = serde_json::from_str(&reviewed).unwrap();
        let prevouts: Vec<Output> = serde_json::from_value(review["prevouts"].clone()).unwrap();
        verify(&transaction, &prevouts).unwrap();
        assert_eq!(
            transaction.inputs[0].witness,
            signed_offer.transaction.inputs[0].witness
        );
        assert_eq!(
            transaction.outputs[0].value - signed_offer.prevout.value,
            9950 * soq_core::fees::ATOMS_PER_SOQ
        );
        assert_eq!(
            transaction.outputs[3],
            soq_core::fees::payment(100 * soq_core::fees::ATOMS_PER_SOQ).unwrap()
        );
        assert_eq!(review["seller_commission_atoms"], "5000000000");
        let discounted = seller
            .create_offer(&lot.to_string(), "10000", true, 75)
            .unwrap();
        let discounted_terms: Offer = serde_json::from_str(&discounted).unwrap();
        verify_offer(&discounted_terms).unwrap();
        let discounted_review = buyer
            .prepare_purchase(&discounted, &funding.to_string(), "0.0012", 50)
            .unwrap();
        let discounted_signed = buyer.sign_review(&discounted_review).unwrap();
        let discounted_tx = Transaction::decode(&hex::decode(discounted_signed).unwrap()).unwrap();
        assert_eq!(
            discounted_tx.outputs[0],
            discounted_terms.transaction.outputs[0]
        );
        assert_eq!(
            discounted_tx.outputs[3],
            soq_core::fees::payment(3_750_000_000).unwrap()
        );
        verify(&discounted_tx, &prevouts).unwrap();

        assert_eq!(transaction.outputs[0], signed_offer.transaction.outputs[0]);
        assert_eq!(
            transaction.outputs[1].script,
            hex::decode(buyer_script).unwrap()
        );
        assert_eq!(
            prevouts.iter().map(|output| output.value).sum::<u64>()
                - transaction
                    .outputs
                    .iter()
                    .map(|output| output.value)
                    .sum::<u64>(),
            120_000
        );
        assert_eq!(review["platform_fee_atoms"], (5_000_000_000u64).to_string());
    }

    #[test]
    fn single_anyonecanpay_is_bound_to_payment() {
        let seller = Wallet::from_seed([7; 32]).unwrap();
        let buyer = Wallet::from_seed([8; 32]).unwrap();
        let prev = Output {
            value: 100000000,
            script: address_script(&seller.receive_address("sq").unwrap(), "sq").unwrap(),
        };
        let mut tx = Transaction {
            version: 2,
            inputs: vec![soq_core::Input {
                previous: soq_core::OutPoint {
                    txid: "ab".repeat(32),
                    vout: 0,
                },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            }],
            outputs: vec![Output {
                value: 200000000,
                script: prev.script.clone(),
            }],
            lock_time: 0,
        };
        let h = tx.sighash_type(0, &prev, 0x83).unwrap();
        let mut sig = seller
            .secret
            .try_sign_with_seed(&[0; 32], &h, &[])
            .unwrap()
            .to_vec();
        sig.push(0x83);
        let mut pk = vec![0];
        pk.extend(seller.public);
        tx.inputs[0].witness = vec![sig, pk];
        verify(&tx, std::slice::from_ref(&prev)).unwrap();
        let buyer_prev = Output {
            value: 300000000,
            script: address_script(&buyer.receive_address("sq").unwrap(), "sq").unwrap(),
        };
        tx.inputs.push(soq_core::Input {
            previous: soq_core::OutPoint {
                txid: "cd".repeat(32),
                vout: 0,
            },
            script: vec![],
            sequence: u32::MAX,
            witness: vec![],
        });
        tx.outputs.push(Output {
            value: 100000000,
            script: buyer_prev.script.clone(),
        });
        let prevs = [prev, buyer_prev];
        buyer.sign(&mut tx, &prevs).unwrap();
        verify(&tx, &prevs).unwrap();
        tx.outputs[0].value -= 1;
        assert!(verify(&tx, &prevs).is_err());
    }
}
