//! Application-ledger fixtures, NOT consensus-valid mined regtest blocks.
use serde_json::{json, Value};
use soq_core::{
    fees::{fee_output, MintKind},
    protocol::Operation,
    OutPoint, Output,
};
use soq_indexer::Store;
fn hash(n: u64) -> String {
    format!("{n:064x}")
}
fn script(owner: u8) -> String {
    format!("5120{}", hex::encode([owner; 32]))
}
fn out(value: u64, script: String, n: usize) -> Value {
    let decimal = format!("{}.{:08}", value / 100000000, value % 100000000);
    json!({"n":n,"value":serde_json::from_str::<Value>(&decimal).unwrap(),"scriptPubKey":{"hex":script}})
}
fn cb(id: u64, owner: u8) -> Value {
    json!({"txid":hash(id),"vin":[{"coinbase":"00"}],"vout":[out(100_000_000_000_000,script(owner),0)]})
}
fn op_tx(id: u64, prev: u64, owner: u8, op: Operation, payment: Option<MintKind>) -> Value {
    let mut outputs = vec![
        out(1000000, script(owner), 0),
        out(0, hex::encode(op.script().unwrap()), 1),
    ];
    if let Some(kind) = payment {
        let fee = fee_output(kind).unwrap();
        outputs.push(out(fee.value, hex::encode(fee.script), 2));
    }
    json!({"txid":hash(id),"vin":[{"txid":hash(prev),"vout":0}],"vout":outputs})
}
struct Chain {
    store: Store,
    height: u64,
    parent: String,
}
impl Chain {
    fn new() -> Self {
        let mut c = Self {
            store: Store::open(":memory:").unwrap(),
            height: 0,
            parent: String::new(),
        };
        c.add(vec![], 1);
        c
    }
    fn add(&mut self, transactions: Vec<Value>, owner: u8) {
        let h = self.height;
        let mut txs = vec![cb(1000 + h, owner)];
        txs.extend(transactions);
        let hash = hash(100000 + h);
        let block =
            json!({"height":h,"hash":hash,"previousblockhash":self.parent,"time":h,"tx":txs});
        self.store.apply(&block).unwrap();
        self.parent = hash;
        self.height += 1;
    }
}
#[test]
fn old_deployments_are_rejected_during_chain_replay() {
    let mut chain = Chain::new();
    let deploy = Operation::Deploy {
        metadata: [1; 32],
        cap: 100,
        limit: 10,
        ticker: *b"TEST\0\0\0\0",
        name_key: Some(soq_core::protocol::name_key("Test Token").unwrap()),
    };
    let mut obsolete = op_tx(110, 1000, 1, deploy.clone(), Some(MintKind::TokenDeploy));
    let mut legacy = deploy.clone();
    if let Operation::Deploy { name_key, .. } = &mut legacy {
        *name_key = None;
    }
    let mut carrier = legacy.script().unwrap();
    carrier[1] += 1;
    carrier.insert(56, 0);
    obsolete["vout"][1]["scriptPubKey"]["hex"] = json!(hex::encode(carrier));
    chain.add(vec![obsolete], 1);
    assert!(chain.store.asset(&hash(110)).is_err());
    assert_eq!(
        chain.store.transaction_status(&hash(110)).unwrap()["asset_operation_error"],
        "deploy length"
    );
    let newer = op_tx(111, 1001, 1, deploy, Some(MintKind::TokenDeploy));
    chain.add(vec![newer], 1);
    assert_eq!(chain.store.asset(&hash(111)).unwrap()["ticker"], "TEST");
    assert_eq!(
        chain
            .store
            .spent_output_script(&hash(1001), 0, &hash(111))
            .unwrap(),
        Some(hex::decode(script(1)).unwrap())
    );
    assert_eq!(
        chain
            .store
            .spent_output_script(&hash(1001), 0, &hash(110))
            .unwrap(),
        None
    );
}

#[test]
fn creation_names_and_tickers_reserve_in_mempool_and_replay_in_block_order() {
    let name = |name| Some(soq_core::protocol::name_key(name).unwrap());
    let token = |metadata, ticker, label| Operation::Deploy {
        metadata: [metadata; 32],
        cap: 100,
        limit: 10,
        ticker,
        name_key: name(label),
    };
    let collection = |metadata, label| Operation::Collection {
        metadata: [metadata; 32],
        maximum: 10,
        name_key: name(label),
    };
    let mut chain = Chain::new();
    let first = op_tx(
        20,
        1000,
        1,
        token(1, *b"TEST\0\0\0\0", "Straße"),
        Some(MintKind::TokenDeploy),
    );
    let key = hex::encode(name("STRASSE").unwrap());
    let other = hex::encode(name("Other").unwrap());
    chain
        .store
        .observe_mempool_transactions(&[hash(20)], std::slice::from_ref(&first))
        .unwrap();
    assert!(chain
        .store
        .check_creation("token", &key, Some("NEXT"))
        .is_err());
    assert!(chain
        .store
        .check_creation("token", &other, Some("TEST"))
        .is_err());
    assert!(chain.store.check_creation("collection", &key, None).is_ok());
    chain.store.observe_mempool(&[]).unwrap();
    assert!(chain
        .store
        .check_creation("token", &key, Some("TEST"))
        .is_ok());
    let duplicate_name = op_tx(
        21,
        1001,
        1,
        token(2, *b"NEXT\0\0\0\0", "STRASSE"),
        Some(MintKind::TokenDeploy),
    );
    let duplicate_ticker = op_tx(
        22,
        1002,
        1,
        token(3, *b"TEST\0\0\0\0", "Other"),
        Some(MintKind::TokenDeploy),
    );
    let first_collection = op_tx(
        23,
        1003,
        1,
        collection(4, "Example"),
        Some(MintKind::Collection),
    );
    let duplicate_collection = op_tx(
        24,
        1004,
        1,
        collection(5, "EXAMPLE"),
        Some(MintKind::Collection),
    );
    for tx in [
        first,
        duplicate_name,
        duplicate_ticker,
        first_collection,
        duplicate_collection,
    ] {
        chain.add(vec![tx], 1);
    }
    assert_eq!(chain.store.assets().unwrap().len(), 1);
    assert_eq!(chain.store.collections().unwrap().len(), 1);
    for invalid in [21, 22, 24] {
        assert!(
            chain.store.transaction_status(&hash(invalid)).unwrap()["asset_operation_error"]
                .is_string()
        );
    }
    chain.store.rollback(1).unwrap();
    assert!(chain
        .store
        .check_creation("token", &key, Some("TEST"))
        .is_ok());
    assert!(chain
        .store
        .check_creation("collection", &hex::encode(name("example").unwrap()), None)
        .is_ok());
}

#[test]
fn discount_requires_six_confirmations_tracks_transfers_and_rolls_back() {
    use soq_core::fees::{discounted_fee, DISCOUNT_NFTS};
    let mut chain = Chain::new();
    let (gold, _) = DISCOUNT_NFTS[0];
    let (silver, _) = DISCOUNT_NFTS[1];
    for (id, prev) in [(gold, 1000), (silver, 1001)] {
        let mut mint = op_tx(
            10,
            prev,
            1,
            Operation::Nft {
                metadata: [2; 32],
                destination: 0,
            },
            Some(MintKind::Nft),
        );
        mint["txid"] = json!(id);
        chain.add(vec![mint], 1);
    }
    while chain.height < 6 {
        chain.add(vec![], 1);
    }
    assert!(chain.store.discount_nfts(&script(1)).unwrap().is_empty());
    chain.add(vec![], 1);
    assert_eq!(
        chain.store.discount_nfts(&script(1)).unwrap(),
        vec![(gold, 75)]
    );
    chain.add(vec![], 1);
    assert_eq!(
        chain.store.discount_nfts(&script(1)).unwrap(),
        vec![(gold, 75), (silver, 50)]
    );
    let transfer = Operation::Transfer {
        asset: soq_core::hash_bytes(gold).unwrap(),
        allocations: vec![(0, 1)],
    };
    let fee = soq_core::fees::transfer_output().unwrap();
    let make_transfer = |id, tier| {
        json!({"txid":hash(id),"vin":[{"txid":gold,"vout":0}],
        "vout":[out(1_000_000,script(2),0),out(0,hex::encode(transfer.script().unwrap()),1),
            out(discounted_fee(fee.value,tier).unwrap(),hex::encode(&fee.script),2)]})
    };
    let height = chain.height;
    let parent = chain.parent.clone();
    let too_much = make_transfer(21, 75);
    chain.add(vec![too_much], 1);
    assert!(
        chain.store.transaction_status(&hash(21)).unwrap()["asset_operation_error"].is_string()
    );
    chain.store.rollback(height).unwrap();
    chain.height = height;
    chain.parent = parent;
    let valid = make_transfer(22, 50);
    chain
        .store
        .observe_mempool_transactions(&[hash(22)], std::slice::from_ref(&valid))
        .unwrap();
    assert_eq!(
        chain.store.discount_nfts(&script(1)).unwrap(),
        vec![(silver, 50)]
    );
    chain.add(vec![valid], 1);
    assert_eq!(
        chain.store.asset_note(&hash(22), 0).unwrap().unwrap().0,
        gold
    );
    assert!(chain.store.discount_nfts(&script(2)).unwrap().is_empty());
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    assert_eq!(
        chain.store.discount_nfts(&script(2)).unwrap(),
        vec![(gold, 75)]
    );
    chain.store.rollback(height).unwrap();
    assert_eq!(
        chain.store.discount_nfts(&script(1)).unwrap(),
        vec![(gold, 75), (silver, 50)]
    );
    assert!(chain.store.discount_nfts(&script(2)).unwrap().is_empty());
}

#[test]
fn settlement_validates_buyer_and_seller_discounts_independently() {
    use soq_core::fees::{dex_commission, discounted_fee, payment, DISCOUNT_NFTS};
    let mut chain = Chain::new();
    let mut gold = op_tx(
        10,
        1000,
        1,
        Operation::Nft {
            metadata: [1; 32],
            destination: 0,
        },
        Some(MintKind::Nft),
    );
    gold["txid"] = json!(DISCOUNT_NFTS[0].0);
    chain.add(vec![gold], 2);
    let mut silver = op_tx(
        11,
        1001,
        2,
        Operation::Nft {
            metadata: [2; 32],
            destination: 0,
        },
        Some(MintKind::Nft),
    );
    silver["txid"] = json!(DISCOUNT_NFTS[1].0);
    chain.add(vec![silver], 1);
    chain.add(
        vec![op_tx(
            12,
            1002,
            1,
            Operation::Deploy {
                metadata: [3; 32],
                cap: 100,
                limit: 10,
                ticker: *b"FEE\0\0\0\0\0",
                name_key: Some(soq_core::protocol::name_key("Fee test").unwrap()),
            },
            Some(MintKind::TokenDeploy),
        )],
        1,
    );
    let asset = soq_core::hash_bytes(&hash(12)).unwrap();
    chain.add(
        vec![op_tx(
            13,
            1003,
            1,
            Operation::Mint {
                token: asset,
                amount: 10,
                destination: 0,
            },
            Some(MintKind::TokenMint),
        )],
        2,
    );
    while chain.height < 10 {
        chain.add(vec![], 2);
    }
    let height = chain.height;
    let parent = chain.parent.clone();
    let price = 200_000_000;
    let base = dex_commission(price).unwrap();
    let seller_fee = discounted_fee(base, 75).unwrap();
    let make_fill = |id, buyer, discount| {
        let pay = payment(seller_fee + discounted_fee(base, discount).unwrap()).unwrap();
        let operation = Operation::Settle {
            asset,
            quantity: 10,
            price,
        };
        json!({"txid":hash(id),"vin":[{"txid":hash(13),"vout":0},{"txid":hash(1004),"vout":0}],"vout":[
            out(1_000_000+price-seller_fee,script(1),0),out(1_000_000,script(buyer),1),
            out(0,hex::encode(operation.script().unwrap()),2),out(pay.value,hex::encode(pay.script),3)]})
    };
    for (id, buyer, discount) in [(14, 2, 75), (15, 3, 50)] {
        chain.add(vec![make_fill(id, buyer, discount)], 2);
        assert!(chain.store.asset_note(&hash(id), 1).unwrap().is_none());
        chain.store.rollback(height).unwrap();
        chain.height = height;
        chain.parent = parent.clone();
    }
    chain.add(vec![make_fill(16, 2, 50)], 2);
    assert_eq!(chain.store.asset_note(&hash(16), 1).unwrap().unwrap().1, 10);
}

#[test]
fn fees_caps_transfer_settlement_and_rollback() {
    let mut c = Chain::new();
    let deploy = Operation::Deploy {
        metadata: [1; 32],
        cap: 10,
        limit: 6,
        ticker: *b"TEST\0\0\0\0",
        name_key: Some(soq_core::protocol::name_key("Test Token").unwrap()),
    };
    c.add(
        vec![op_tx(10, 1000, 1, deploy, Some(MintKind::TokenDeploy))],
        1,
    );
    let mint = Operation::Mint {
        token: soq_core::hash_bytes(&hash(10)).unwrap(),
        amount: 6,
        destination: 0,
    };
    c.add(vec![op_tx(11, 1001, 1, mint, Some(MintKind::TokenMint))], 1);
    assert_eq!(c.store.assets().unwrap()[0]["minted"], "6");
    let excessive = Operation::Mint {
        token: soq_core::hash_bytes(&hash(10)).unwrap(),
        amount: 5,
        destination: 0,
    };
    c.add(
        vec![op_tx(12, 1002, 1, excessive, Some(MintKind::TokenMint))],
        1,
    );
    assert_eq!(c.store.assets().unwrap()[0]["minted"], "6");
    let final_mint = Operation::Mint {
        token: soq_core::hash_bytes(&hash(10)).unwrap(),
        amount: 4,
        destination: 0,
    };
    c.add(
        vec![op_tx(13, 1003, 1, final_mint, Some(MintKind::TokenMint))],
        1,
    );
    assert_eq!(c.store.assets().unwrap()[0]["minted"], "10");
    let point = OutPoint {
        txid: hash(11),
        vout: 0,
    };
    let payout = Output {
        value: 200_000_000,
        script: hex::decode(script(1)).unwrap(),
    };
    let offer_id = hash(12345);
    assert!(!c.store.has_listing_receipt(&offer_id, &script(1)).unwrap());
    let receipt = op_tx(
        15,
        1005,
        1,
        Operation::Listing {
            offer: soq_core::hash_bytes(&offer_id).unwrap(),
        },
        Some(MintKind::Listing),
    );
    c.add(vec![receipt], 1);
    assert!(c.store.has_listing_receipt(&offer_id, &script(1)).unwrap());
    assert!(!c.store.has_listing_receipt(&offer_id, &script(2)).unwrap());
    c.store
        .register_listing(
            &offer_id,
            "fixture",
            &point,
            &hash(10),
            soq_indexer::ListingTerms {
                quantity: 6,
                price: 200000000,
            },
            &payout,
        )
        .unwrap();
    assert_eq!(c.store.listings().unwrap().len(), 1);
    let before = c.store.token_markets(5, Some(&hash(10))).unwrap();
    assert_eq!(before["markets"][hash(10)]["holders"], 1);
    assert!(before["markets"][hash(10)]["last"].is_null());
    assert_eq!(before["history"], json!([]));

    let reserved_split = soq_core::Transaction {
        version: 2,
        inputs: [hash(13), hash(11)]
            .into_iter()
            .map(|txid| soq_core::Input {
                previous: OutPoint { txid, vout: 0 },
                script: vec![],
                sequence: u32::MAX,
                witness: vec![],
            })
            .collect(),
        outputs: vec![
            Output {
                value: 1_000_000,
                script: hex::decode(script(1)).unwrap(),
            },
            Output {
                value: 0,
                script: Operation::Transfer {
                    asset: soq_core::hash_bytes(&hash(10)).unwrap(),
                    allocations: vec![(0, 10)],
                }
                .script()
                .unwrap(),
            },
        ],
        lock_time: 0,
    };
    assert_eq!(
        c.store.preflight(&reserved_split).unwrap_err().to_string(),
        "cancel the signed listing before transferring that lot"
    );
    assert_eq!(c.store.listings().unwrap()[0]["txid"], hash(11));

    let transfer = Operation::Settle {
        asset: soq_core::hash_bytes(&hash(10)).unwrap(),
        quantity: 6,
        price: 200_000_000,
    };
    let purchase = json!({"txid":hash(14),"vin":[{"txid":hash(11),"vout":0},{"txid":hash(1004),"vout":0}],"vout":[out(payout.value,script(1),0),out(1_000_000,script(2),1),out(0,hex::encode(transfer.script().unwrap()),2),out(2_000_000,hex::encode(soq_core::fees::transfer_output().unwrap().script),3)]});
    c.add(vec![purchase], 1);
    assert_eq!(
        c.store.asset_balance(&script(2)).unwrap()[0]["quantity"],
        "6"
    );
    assert!(c.store.listings().unwrap().is_empty());
    assert_eq!(c.store.trades().unwrap().len(), 1);
    let snapshot = c.store.token_markets(6, Some(&hash(10))).unwrap();
    let market = &snapshot["markets"][hash(10)];
    assert_eq!(
        market["last"],
        json!({"price_atoms":"200000000","quantity":"6"})
    );
    assert!(market["previous"].is_null());
    assert_eq!(market["volume_day_atoms"], "200000000");
    assert_eq!(market["volume_total_atoms"], "200000000");
    assert_eq!(market["circulating_units"], "10");
    assert_eq!(market["holders"], 2);
    assert_eq!(snapshot["history"][0]["txid"], hash(14));
    let aged = c.store.token_markets(86_406, None).unwrap();
    let market = &aged["markets"][hash(10)];
    assert_eq!(market["last"], market["previous"]);
    assert_eq!(market["volume_day_atoms"], "0");
    c.store.rollback(6).unwrap();
    c.height = 6;
    c.parent = hash(100005);
    assert!(c.store.trades().unwrap().is_empty());
    assert!(c.store.token_markets(6, None).unwrap()["markets"][hash(10)]["last"].is_null());
    assert_eq!(c.store.listings().unwrap().len(), 1);
    assert!(c.store.asset_balance(&script(2)).unwrap().is_empty());
    assert!(c.store.has_listing_receipt(&offer_id, &script(1)).unwrap());
    let cancel = Operation::Cancel {
        asset: soq_core::hash_bytes(&hash(10)).unwrap(),
        quantity: 6,
    };
    let cancellation = json!({"txid":hash(16),"vin":[{"txid":hash(11),"vout":0}],
        "vout":[out(1_000_000,script(1),0),out(0,hex::encode(cancel.script().unwrap()),1)]});
    c.store
        .observe_mempool_transactions(&[hash(16)], std::slice::from_ref(&cancellation))
        .unwrap();
    assert_eq!(
        c.store.mempool_spender(&hash(11), 0).unwrap(),
        Some(hash(16))
    );
    assert!(c.store.has_output_listing(&hash(11), 0).unwrap());
    assert!(c.store.listings().unwrap().is_empty());
    assert!(c.store.asset_note(&hash(16), 0).unwrap().is_none());
    assert_eq!(
        c.store.asset_balance(&script(1)).unwrap()[0]["quantity"],
        "10"
    );
    c.store.observe_mempool(&[]).unwrap();
    assert_eq!(c.store.mempool_spender(&hash(11), 0).unwrap(), None);
    assert_eq!(c.store.listings().unwrap().len(), 1);
    c.store
        .observe_mempool_transactions(&[hash(16)], std::slice::from_ref(&cancellation))
        .unwrap();
    c.add(vec![cancellation], 1);
    assert_eq!(c.store.asset_note(&hash(16), 0).unwrap().unwrap().1, 6);
    assert!(!c.store.has_output_listing(&hash(16), 0).unwrap());
    assert_eq!(c.store.mempool_spender(&hash(11), 0).unwrap(), None);
    assert!(c.store.listings().unwrap().is_empty());
    assert!(c.store.trades().unwrap().is_empty());
    c.store.verify().unwrap();
}
#[test]
fn unpaid_and_unauthorized_collection_mints_never_credit() {
    let mut c = Chain::new();
    c.add(
        vec![op_tx(
            1,
            1000,
            1,
            Operation::Collection {
                metadata: [4; 32],
                maximum: 1,
                name_key: Some(soq_core::protocol::name_key("Test Collection").unwrap()),
            },
            Some(MintKind::Collection),
        )],
        2,
    );
    let nft = Operation::CollectionNft {
        collection: soq_core::hash_bytes(&hash(1)).unwrap(),
        metadata: [9; 32],
        destination: 0,
    };
    c.add(vec![op_tx(2, 1001, 2, nft.clone(), Some(MintKind::Nft))], 1);
    assert!(c.store.assets().unwrap().is_empty());
    c.add(vec![op_tx(3, 1002, 1, nft.clone(), None)], 1);
    assert!(c.store.assets().unwrap().is_empty());
    c.add(vec![op_tx(4, 1003, 1, nft.clone(), Some(MintKind::Nft))], 1);
    assert_eq!(c.store.assets().unwrap().len(), 1);
    c.add(vec![op_tx(5, 1004, 1, nft, Some(MintKind::Nft))], 1);
    assert_eq!(c.store.assets().unwrap().len(), 1);
    assert_eq!(c.store.collections().unwrap()[0]["minted"], 1);
}
#[test]
fn fingerprints_are_canonical_after_confirmations_and_reorganizations() {
    let mut c = Chain::new();
    let request = Operation::MapRequest { destination: 0 };
    let first = op_tx(900, 1000, 2, request.clone(), Some(MintKind::Map));
    let mut second = op_tx(100, 1001, 1, request.clone(), Some(MintKind::Map)); // Same-block funding below is the coinbase at height 1.
    second["vin"][0]["txid"] = json!(hash(1001));
    c.add(vec![first, second], 1);
    let unpaid = op_tx(99, 1002, 1, request, None);
    c.add(vec![unpaid], 1);
    while c.height <= 10 {
        c.add(vec![], 1);
    }
    assert!(c.store.maps().unwrap().is_empty());
    c.add(vec![], 1);
    assert_eq!(c.store.maps().unwrap().len(), 1);
    assert_eq!(
        c.store.asset_note(&hash(900), 0).unwrap().unwrap().0,
        hash(900)
    );
    assert!(c.store.asset_note(&hash(100), 0).unwrap().is_none());
    c.add(vec![], 1);
    assert_eq!(c.store.maps().unwrap().len(), 2);
    let before = c.store.map_artwork(6).unwrap();
    assert_eq!(before["parameters"]["height"], 6);
    assert_eq!(before["parameters"]["creator_script"], script(1));
    assert_eq!(before["parameters"]["txid"], hash(900));
    assert_eq!(before["parameters"]["vout"], 0);
    assert_eq!(before["type"], "soq-fingerprint");
    assert_eq!(c.store.asset(&hash(900)).unwrap()["kind"], "fingerprint");
    assert!(c
        .store
        .asset_balance(&script(2))
        .unwrap()
        .iter()
        .any(|note| note["asset"] == hash(900) && note["quantity"] == "1"));
    c.store.rollback(6).unwrap();
    assert!(c.store.maps().unwrap().is_empty());
    c.height = 6;
    c.parent = hash(100005);
    while c.height <= 12 {
        let h = c.height;
        let txs = vec![cb(1000 + h, 1)];
        let block_hash = hash(200000 + h);
        let b =
            json!({"height":h,"hash":block_hash,"previousblockhash":c.parent,"time":h,"tx":txs});
        c.store.apply(&b).unwrap();
        c.parent = block_hash;
        c.height += 1;
    }
    let after = c.store.map_artwork(6).unwrap();
    assert_eq!(after["source_reorganized"], true);
    assert_eq!(
        after["prior_block_hashes"][0],
        before["parameters"]["block_hash"]
    );
    let image = c
        .store
        .map_content(after["content_sha256"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        hex::encode(soq_core::sha256(&image)),
        after["content_sha256"].as_str().unwrap()
    );
    assert_ne!(
        before["parameters"]["block_hash"],
        after["parameters"]["block_hash"]
    );
    let creator = hex::decode(script(1)).unwrap().try_into().unwrap();
    let (_, reproduced) =
        soq_fingerprints::canonical(&creator, &hash(900), 0, &hash(200006), 6).unwrap();
    assert_eq!(image, reproduced);
    assert_ne!(before["content_sha256"], after["content_sha256"]);
    let asset = c.store.asset(&hash(900)).unwrap();
    let metadata = c
        .store
        .map_content(asset["metadata_sha256"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&metadata).unwrap()["content_sha256"],
        after["content_sha256"]
    );
    assert_eq!(c.store.maps().unwrap().len(), 2);
    assert!(c.store.asset_note(&hash(99), 0).unwrap().is_none());
    c.store.verify().unwrap();
}
#[test]
fn signed_pending_data_survives_restart_without_crediting_assets() {
    let path = std::env::temp_dir().join(format!("soq-pending-test-{}.sqlite", std::process::id()));
    {
        let s = Store::open(&path).unwrap();
        s.record_pending(&hash(99), "public-signed-transaction")
            .unwrap();
        s.pending_state(&hash(99), "unknown").unwrap();
    }
    {
        let s = Store::open(&path).unwrap();
        assert_eq!(s.pending(&hash(99)).unwrap().as_deref(), Some("unknown"));
        assert!(s.assets().unwrap().is_empty());
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn wallet_history_reports_real_confirmed_movements() {
    let mut c = Chain::new();
    c.add(
        vec![op_tx(
            900,
            1000,
            1,
            Operation::Collection {
                metadata: [1; 32],
                maximum: 10,
                name_key: Some(soq_core::protocol::name_key("History Collection").unwrap()),
            },
            None,
        )],
        1,
    );
    let history = c.store.history(&script(1)).unwrap();
    let operation = history.iter().find(|t| t["txid"] == hash(900)).unwrap();
    assert_eq!(operation["received_atoms"], "1000000");
    assert_eq!(operation["spent_atoms"], "100000000000000");
    assert_eq!(operation["confirmations"], 1);
    assert!(operation["marketplace_action"].is_null());
    assert!(c.store.history(&script(2)).unwrap().is_empty());
}

#[test]
fn maps_survive_restart_and_follow_the_reorganized_transaction_order() {
    let path = std::env::temp_dir().join(format!("soq-map-fifo-{}.sqlite", std::process::id()));
    let mut chain = Chain {
        store: Store::open(&path).unwrap(),
        height: 0,
        parent: String::new(),
    };
    while chain.height < 7 {
        chain.add(vec![], 1);
    }
    let request = Operation::SoquMapRequest { destination: 0 };
    let first = op_tx(900, 1001, 1, request.clone(), Some(MintKind::Map));
    let second = op_tx(100, 1002, 2, request.clone(), Some(MintKind::Map));
    let immature = op_tx(800, 1006, 1, request.clone(), Some(MintKind::Map));
    let unpaid = op_tx(700, 1000, 1, request, None);
    chain.add(vec![first.clone(), second.clone(), immature, unpaid], 1);
    assert!(chain.store.protected(&hash(900), 0).unwrap());
    assert!(!chain.store.protected(&hash(800), 0).unwrap());
    assert!(!chain.store.protected(&hash(700), 0).unwrap());
    assert_eq!(
        chain.store.transaction_status(&hash(900)).unwrap()["map"]["assigned_height"],
        Value::Null
    );
    while chain.height <= 12 {
        chain.add(vec![], 1);
    }
    assert_eq!(
        chain.store.transaction_status(&hash(900)).unwrap()["map"]["assigned_height"],
        12
    );
    assert_eq!(
        chain.store.transaction_status(&hash(100)).unwrap()["map"]["assigned_height"],
        13
    );
    assert!(chain.store.generated_artworks(true).unwrap().is_empty());

    drop(chain.store);
    chain.store = Store::open(&path).unwrap();
    while chain.height <= 18 {
        chain.add(vec![], 1);
    }
    let original = chain.store.generated_artwork(12, true).unwrap();
    assert_eq!(original["asset_origin_txid"], hash(900));
    assert_eq!(original["name"], "12.soqumap");
    assert!(chain.store.maps().unwrap().is_empty());
    let map_asset = chain.store.asset(&hash(900)).unwrap();
    assert_eq!(map_asset["kind"], "map");
    assert!(chain
        .store
        .asset_balance(&script(1))
        .unwrap()
        .iter()
        .any(|note| note["asset"] == hash(900) && note["quantity"] == "1"));
    let metadata = chain
        .store
        .map_content(map_asset["metadata_sha256"].as_str().unwrap())
        .unwrap();
    let metadata: Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata["content_sha256"], original["content_sha256"]);
    assert_eq!(
        chain.store.transaction_status(&hash(900)).unwrap()["map"]["revealed"],
        true
    );
    let image = chain
        .store
        .map_content(original["content_sha256"].as_str().unwrap())
        .unwrap();
    assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");

    chain.store.rollback(7).unwrap();
    assert!(chain.store.generated_artworks(true).unwrap().is_empty());
    assert!(!chain.store.protected(&hash(900), 0).unwrap());
    chain.parent = hash(100006);
    for height in 7..=18 {
        let mut transactions = vec![cb(2000 + height, 1)];
        if height == 7 {
            transactions.extend([second.clone(), first.clone()]);
        }
        let block_hash = hash(200000 + height);
        chain
            .store
            .apply(
                &json!({"height":height,"hash":block_hash,"previousblockhash":chain.parent,
            "time":height,"tx":transactions}),
            )
            .unwrap();
        chain.parent = block_hash;
    }
    assert_eq!(
        chain.store.generated_artwork(12, true).unwrap()["asset_origin_txid"],
        hash(100)
    );
    let reassigned = chain.store.generated_artwork(13, true).unwrap();
    assert_eq!(reassigned["asset_origin_txid"], hash(900));
    assert_eq!(reassigned["source_reorganized"], true);
    assert_ne!(reassigned["content_sha256"], original["content_sha256"]);
    assert!(chain
        .store
        .map_content(original["content_sha256"].as_str().unwrap())
        .is_err());
    chain.store.verify().unwrap();
    drop(chain.store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mempool_funding_projects_unspent_outputs_without_counting_asset_anchors() {
    let mut chain = Chain::new();
    let funding = op_tx(
        100,
        1000,
        1,
        Operation::SoquMapRequest { destination: 0 },
        Some(MintKind::Map),
    );
    let mut funding = funding;
    funding["vout"]
        .as_array_mut()
        .unwrap()
        .push(out(1_000_000_000, script(1), 3));
    let child = json!({"txid":hash(101),"vin":[{"txid":hash(100),"vout":3}],
        "vout":[out(900_000_000,script(1),0)]});
    chain
        .store
        .observe_mempool_transactions(&[hash(100), hash(101)], &[funding, child])
        .unwrap();
    let pending = chain.store.mempool_funding(&script(1)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["confirmations"], 0);
    assert_eq!(pending[0]["txid"], hash(101));
    assert!(chain.store.mempool_spends(&hash(1000), 0).unwrap());
    // Wallet totals include the map anchor and only the unspent end of the change chain.
    assert_eq!(
        chain.store.wallet_funding(&script(1), "main").unwrap()["pending_atoms"],
        "901000000"
    );

    chain
        .store
        .observe_mempool_transactions(&[hash(100)], &[])
        .unwrap();
    let pending = chain.store.mempool_funding(&script(1)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["vout"], 3);
    chain.store.observe_mempool_transactions(&[], &[]).unwrap();
    assert!(chain.store.mempool_funding(&script(1)).unwrap().is_empty());
    assert!(!chain.store.mempool_spends(&hash(1000), 0).unwrap());
}

#[test]
fn wallet_balances_track_mempool_change_confirmation_and_rollback() {
    use soq_core::{Input, Transaction};
    const SOQ: u64 = 100_000_000;
    let mut chain = Chain::new();
    let owner = script(2);
    let mut anchor = op_tx(
        90,
        1000,
        2,
        Operation::Nft {
            metadata: [3; 32],
            destination: 0,
        },
        Some(MintKind::Nft),
    );
    anchor["vout"][0] = out(SOQ, owner.clone(), 0);
    chain.add(vec![anchor], 1);
    chain.add(
        vec![json!({
            "txid":hash(91),"vin":[{"txid":hash(1001),"vout":0}],
            "vout":[out(100*SOQ,owner.clone(),0)]
        })],
        1,
    );
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    let check = |store: &Store, total: u64, assets: u64, available: u64, pending: u64| {
        let balances = store.wallet_funding(&owner, "main").unwrap();
        assert_eq!(balances["confirmed_atoms"], (total * SOQ).to_string());
        assert_eq!(balances["asset_atoms"], (assets * SOQ).to_string());
        assert_eq!(balances["available_atoms"], (available * SOQ).to_string());
        assert_eq!(balances["pending_atoms"], (pending * SOQ).to_string());
    };
    check(&chain.store, 101, 1, 100, 0);

    let tx = Transaction {
        version: 2,
        inputs: vec![Input {
            previous: OutPoint {
                txid: hash(91),
                vout: 0,
            },
            script: vec![],
            sequence: u32::MAX,
            witness: vec![],
        }],
        outputs: vec![
            Output {
                value: 10 * SOQ,
                script: hex::decode(script(3)).unwrap(),
            },
            Output {
                value: 90 * SOQ,
                script: hex::decode(&owner).unwrap(),
            },
        ],
        lock_time: 0,
    };
    let id = tx.txid().unwrap();
    chain
        .store
        .record_submission(&tx, &hex::encode(tx.encode(true).unwrap()))
        .unwrap();
    chain.store.accept_submission(&tx).unwrap();
    check(&chain.store, 1, 1, 0, 90);
    let history = chain.store.history(&owner).unwrap();
    assert_eq!(history[0]["txid"], id);
    assert_eq!(history[0]["confirmations"], 0);
    assert_eq!(history[0]["received_atoms"], (90 * SOQ).to_string());

    let block = chain.height;
    let body = json!({
        "txid":id,"vin":[{"txid":hash(91),"vout":0}],
        "vout":[out(10*SOQ,script(3),0),out(90*SOQ,owner.clone(),1)]
    });
    chain.add(vec![body.clone()], 1);
    for confirmations in 1..6 {
        check(&chain.store, 1, 1, 0, 90);
        assert_eq!(
            chain.store.transaction_status(&id).unwrap()["confirmations"],
            confirmations
        );
        assert_eq!(
            chain
                .store
                .history(&owner)
                .unwrap()
                .iter()
                .filter(|row| row["txid"] == id)
                .count(),
            1
        );
        chain.add(vec![], 1);
    }
    check(&chain.store, 91, 1, 90, 0);
    assert_eq!(
        chain.store.transaction_status(&id).unwrap()["confirmations"],
        6
    );

    chain.store.rollback(block).unwrap();
    assert_eq!(
        chain.store.transaction_status(&id).unwrap()["state"],
        "reorganized"
    );
    chain
        .store
        .observe_mempool_transactions(std::slice::from_ref(&id), &[body])
        .unwrap();
    check(&chain.store, 1, 1, 0, 90);
    assert_eq!(
        chain.store.transaction_status(&id).unwrap()["state"],
        "mempool"
    );
    chain.store.observe_mempool(&[]).unwrap();
    check(&chain.store, 101, 1, 100, 0);
    assert_eq!(
        chain.store.transaction_status(&id).unwrap()["state"],
        "not_in_mempool"
    );
}

fn fatcat_claim(id: u64, source: u64, input: usize, serial: u16, owner: u8) -> Value {
    let op = Operation::FatcatClaim {
        serial,
        destination: 0,
    };
    json!({"txid":hash(id),"vin":[{"txid":hash(source),"vout":input}],
        "vout":[out(1_000_000,script(owner),0),out(0,hex::encode(op.script().unwrap()),1)]})
}

fn fatcat_mint(id: u64, claim: u64, funding: u64, input: usize, price: u64, owner: u8) -> Value {
    let op = Operation::FatcatMint { destination: 0 };
    let fee = soq_core::fees::payment(price).unwrap();
    json!({"txid":hash(id),"vin":[{"txid":hash(claim),"vout":0},{"txid":hash(funding),"vout":input}],
        "vout":[out(1_000_000,script(owner),0),out(0,hex::encode(op.script().unwrap()),1),out(fee.value,hex::encode(fee.script),2)]})
}

#[test]
fn fatcat_claim_payment_reveal_marketplace_restart_and_reorg() {
    use soq_core::protocol::FATCAT_PRICE;
    let path = std::env::temp_dir().join(format!("soq-fatcat-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut chain = Chain {
        store: Store::open(&path).unwrap(),
        height: 0,
        parent: String::new(),
    };
    chain.add(vec![], 1);
    chain
        .store
        .configure_regtest_fatcat_issuer(&script(1))
        .unwrap();
    chain.add(
        vec![
            json!({"txid":hash(100),"vin":[{"txid":hash(1000),"vout":0}],"vout":[
                out(1_000_001,script(1),0),out(1_000_001,script(1),1),out(1_000_001,script(1),2),
                out(FATCAT_PRICE+1_000_000,script(2),3),out(FATCAT_PRICE+1_000_000,script(2),4),
                out(200_000_000,script(2),5),out(200_000_000,script(3),6),out(500_000_000,script(4),7)
            ]}),
        ],
        1,
    );
    chain.add(
        vec![
            fatcat_claim(101, 100, 0, 1, 2),
            fatcat_claim(102, 100, 1, 1, 2),
            fatcat_claim(103, 100, 2, 2, 2),
        ],
        1,
    );
    assert!(chain.store.protected(&hash(101), 0).unwrap());
    assert!(
        chain.store.transaction_status(&hash(102)).unwrap()["asset_operation_error"]
            .as_str()
            .unwrap()
            .contains("already issued")
    );
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    let paid_height = chain.height;
    let paid = fatcat_mint(106, 101, 100, 3, FATCAT_PRICE, 2);
    let discounted = fatcat_mint(107, 103, 100, 4, FATCAT_PRICE / 4, 2);
    chain.add(vec![paid, discounted], 1);
    assert!(
        chain.store.transaction_status(&hash(107)).unwrap()["asset_operation_error"]
            .as_str()
            .unwrap()
            .contains("2,000")
    );
    assert!(chain.store.protected(&hash(106), 0).unwrap());
    for count in 1..=5 {
        assert_eq!(
            chain.store.transaction_status(&hash(106)).unwrap()["confirmations"],
            count
        );
        assert!(chain.store.asset(&hash(106)).is_err());
        assert!(chain.store.fatcat_gallery().unwrap().is_empty());
        chain.add(vec![], 1);
    }
    let tip = chain.parent.clone();
    chain.store = Store::open(&path).unwrap();
    let status = chain.store.transaction_status(&hash(106)).unwrap();
    assert_eq!(status["confirmations"], 6);
    assert_eq!(status["fatcat"]["canonical_reveal"], true);
    assert_eq!(status["fatcat"]["reveal_block"], tip);
    let nft = chain.store.asset(&hash(106)).unwrap();
    assert_eq!(nft["standard"], "SOQ-721");
    assert!(nft["collection"].is_null());
    let gallery = chain.store.fatcat_gallery().unwrap();
    assert_eq!(gallery[0]["owner_script"], script(2));
    let metadata = chain
        .store
        .fatcat_content(nft["metadata_sha256"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        hex::encode(soq_core::sha256(&metadata)),
        nft["metadata_sha256"]
    );
    assert_eq!(
        chain.store.asset_balance(&script(2)).unwrap()[0]["quantity"],
        "1"
    );

    let op = Operation::Transfer {
        asset: soq_core::hash_bytes(&hash(106)).unwrap(),
        allocations: vec![(0, 1)],
    };
    let fee = soq_core::fees::transfer_output().unwrap();
    chain.add(vec![json!({"txid":hash(110),"vin":[{"txid":hash(106),"vout":0},{"txid":hash(100),"vout":5}],
        "vout":[out(1_000_000,script(3),0),out(0,hex::encode(op.script().unwrap()),1),out(fee.value,hex::encode(fee.script),2)]})],1);
    assert_eq!(
        chain.store.fatcat_gallery().unwrap()[0]["owner_script"],
        script(3)
    );

    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    let offer = hash(9999);
    let receipt = Operation::Listing {
        offer: soq_core::hash_bytes(&offer).unwrap(),
    };
    let listing_fee = soq_core::fees::fee_output(MintKind::Listing).unwrap();
    chain.add(vec![json!({"txid":hash(111),"vin":[{"txid":hash(100),"vout":6}],
        "vout":[out(0,hex::encode(receipt.script().unwrap()),0),out(listing_fee.value,hex::encode(listing_fee.script),1)]})],1);
    let point = OutPoint {
        txid: hash(110),
        vout: 0,
    };
    let payout = Output {
        value: 201_000_000,
        script: hex::decode(script(3)).unwrap(),
    };
    chain
        .store
        .register_listing(
            &offer,
            "fixture",
            &point,
            &hash(106),
            soq_indexer::ListingTerms {
                quantity: 1,
                price: 200_000_000,
            },
            &payout,
        )
        .unwrap();
    assert_eq!(chain.store.listings().unwrap()[0]["fee_paid"], true);
    let settlement = Operation::Settle {
        asset: soq_core::hash_bytes(&hash(106)).unwrap(),
        quantity: 1,
        price: 200_000_000,
    };
    let fee = soq_core::fees::payment(soq_core::fees::PURCHASE_FEE).unwrap();
    chain.add(vec![json!({"txid":hash(112),"vin":[{"txid":hash(110),"vout":0},{"txid":hash(100),"vout":7}],
        "vout":[out(payout.value,script(3),0),out(1_000_000,script(4),1),
        out(0,hex::encode(settlement.script().unwrap()),2),out(fee.value,hex::encode(fee.script),3)]})],1);
    assert_eq!(
        chain.store.fatcat_gallery().unwrap()[0]["owner_script"],
        script(4)
    );
    assert!(chain.store.listings().unwrap().is_empty());
    assert_eq!(chain.store.trades().unwrap().len(), 1);
    assert_eq!(
        chain.store.token_markets(0, None).unwrap()["collection_volume_atoms"]["fatcats"],
        "200000000"
    );

    chain.store.rollback(paid_height + 5).unwrap();
    assert_eq!(chain.store.fatcat_state().unwrap(), (1, true));
    let suspended = chain.store.transaction_status(&hash(106)).unwrap();
    assert_eq!(suspended["fatcat"]["number"], status["fatcat"]["number"]);
    assert_eq!(suspended["fatcat"]["canonical_reveal"], false);
    assert!(chain.store.asset(&hash(106)).is_err());
    assert!(chain.store.fatcat_gallery().unwrap().is_empty());
    chain.store.verify().unwrap();
    drop(chain);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fatcat_full_supply_is_unique_with_all_claims_in_one_wallet_and_one_block() {
    use soq_core::protocol::{FATCAT_PRICE, FATCAT_SUPPLY};
    let mut chain = Chain::new();
    chain
        .store
        .configure_regtest_fatcat_issuer(&script(1))
        .unwrap();
    let count = usize::from(FATCAT_SUPPLY);
    let funding = (0..count * 2)
        .map(|i| {
            out(
                FATCAT_PRICE + 1_000_000,
                script(if i < count { 1 } else { 2 }),
                i,
            )
        })
        .collect::<Vec<_>>();
    chain.add(
        vec![json!({"txid":hash(199),"vin":[{"txid":hash(1000),"vout":0}],"vout":funding})],
        1,
    );
    chain.add(
        (0..count)
            .map(|i| fatcat_claim(10_000 + i as u64, 199, i, i as u16 + 1, 2))
            .collect(),
        1,
    );
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    chain.add(
        (0..count)
            .map(|i| {
                fatcat_mint(
                    20_000 + i as u64,
                    10_000 + i as u64,
                    199,
                    count + i,
                    FATCAT_PRICE,
                    2,
                )
            })
            .collect(),
        1,
    );
    assert!(chain.store.fatcat_gallery().unwrap().is_empty());
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    let gallery = chain.store.fatcat_gallery().unwrap();
    assert_eq!(gallery.len(), count);
    assert_eq!(chain.store.fatcat_state().unwrap(), (FATCAT_SUPPLY, false));
    let mut variants = std::collections::BTreeSet::new();
    let mut numbers = std::collections::BTreeSet::new();
    for asset in &gallery {
        assert!(variants.insert(asset["variant"].as_u64().unwrap()));
        assert!(numbers.insert(asset["number"].as_u64().unwrap()));
        assert_eq!(asset["owner_script"], script(2));
    }
    assert_eq!(variants, (0..count as u64).collect());
    assert_eq!(numbers, (1..=count as u64).collect());
    assert!(gallery
        .iter()
        .any(|asset| asset["variant"].as_u64().unwrap() >= 1000
            && asset["number"].as_u64().unwrap() < 1001));
}


#[test]
fn direct_fatcats_allocate_at_six_in_block_order_and_stop_at_supply() {
    use soq_core::protocol::{FATCAT_PRICE, FATCAT_SUPPLY};
    let mut chain = Chain::new();
    let count = usize::from(FATCAT_SUPPLY);
    let funding = (0..=count)
        .map(|i| out(FATCAT_PRICE + 2_000_000, script(2), i))
        .collect::<Vec<_>>();
    chain.add(vec![json!({"txid":hash(199),"vin":[{"txid":hash(1000),"vout":0}],"vout":funding})], 1);
    for _ in 0..5 {
        chain.add(vec![], 1);
    }
    let source = |i: usize| {
        let mut tx = op_tx(
            20_000 + i as u64, 199, 2, Operation::FatcatRequest { destination: 0 },
            Some(MintKind::Fatcat),
        );
        tx["vin"][0]["vout"] = json!(i);
        tx
    };
    chain.add((0..count-1).map(source).collect(), 1);
    assert_eq!(chain.store.fatcat_availability().unwrap(), (1, 0, false));
    let last = source(count - 1);
    let competing = soq_core::Transaction {
        version: 2,
        inputs: vec![soq_core::Input {
            previous: OutPoint { txid: hash(199), vout: count as u32 },
            script: vec![], sequence: u32::MAX, witness: vec![],
        }],
        outputs: vec![
            Output { value: 1_000_000, script: hex::decode(script(2)).unwrap() },
            Output { value: 0, script: Operation::FatcatRequest { destination: 0 }.script().unwrap() },
            soq_core::fees::fee_output(MintKind::Fatcat).unwrap(),
        ],
        lock_time: 0,
    };
    chain.store.preflight(&competing).unwrap();
    chain.store.observe_mempool_transactions(
        &[hash(20_000 + count as u64 - 1)], std::slice::from_ref(&last),
    ).unwrap();
    assert_eq!(chain.store.fatcat_availability().unwrap(), (0, 0, false));
    assert_eq!(chain.store.preflight(&competing).unwrap_err().to_string(), "No FATCAT mints remain available");
    chain.store.observe_mempool(&[]).unwrap();
    assert_eq!(chain.store.fatcat_availability().unwrap(), (1, 0, false));
    chain.store.preflight(&competing).unwrap();
    let overflow = hash(20_000 + count as u64);
    chain.add(vec![last, source(count)], 1);
    assert_eq!(chain.store.transaction_status(&overflow).unwrap()["asset_operation_error"], "No FATCAT mints remain available");

    for _ in 0..4 {
        assert!(chain.store.fatcat_gallery().unwrap().is_empty());
        chain.add(vec![], 1);
    }
    assert_eq!(chain.store.fatcat_gallery().unwrap().len(), count - 1);
    chain.add(vec![], 1);
    let gallery = chain.store.fatcat_gallery().unwrap();
    assert_eq!(gallery.len(), count);
    let numbers: std::collections::BTreeSet<_> = gallery.iter().map(|asset| {
        let number = asset["number"].as_u64().unwrap();
        assert_eq!(asset["variant"], number - 1);
        assert_eq!(asset["owner_script"], script(2));
        number
    }).collect();
    assert_eq!(numbers, (1..=u64::from(FATCAT_SUPPLY)).collect());
    assert_eq!(chain.store.fatcat_availability().unwrap(), (0, FATCAT_SUPPLY, false));
    let id = &gallery[0]["id"];
    assert_eq!(chain.store.asset(id.as_str().unwrap()).unwrap()["virtual_collection"], "fatcats");
    assert_eq!(chain.store.assets().unwrap().iter().find(|asset| asset["id"] == *id).unwrap()["virtual_collection"], "fatcats");
    let reveal = chain.height - 1;
    chain.store.rollback(reveal).unwrap();
    assert_eq!(chain.store.fatcat_state().unwrap(), (FATCAT_SUPPLY, true));
}
