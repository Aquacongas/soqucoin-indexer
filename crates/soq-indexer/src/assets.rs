//! Persistent canonical-history application ledger. No database-created ownership.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
pub use soq_core::protocol::Operation;
use soq_core::Output;
pub const CONFIRMATIONS: u64 = Operation::MINT_CONFIRMATIONS;
pub fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS assets(id TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,kind TEXT NOT NULL,metadata TEXT NOT NULL,cap INTEGER NOT NULL,mint_limit INTEGER NOT NULL,ticker TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS collections(id TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,metadata TEXT NOT NULL,creator_script TEXT NOT NULL,maximum INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS creation_names(kind TEXT NOT NULL,name_key TEXT NOT NULL,txid TEXT UNIQUE NOT NULL REFERENCES transactions(txid) ON DELETE CASCADE,PRIMARY KEY(kind,name_key));
CREATE TABLE IF NOT EXISTS collection_nfts(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,collection TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS asset_mints(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,asset TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,quantity INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS asset_notes(txid TEXT NOT NULL,n INTEGER NOT NULL,asset TEXT NOT NULL REFERENCES assets(id) ON DELETE CASCADE,quantity INTEGER NOT NULL,PRIMARY KEY(txid,n),FOREIGN KEY(txid,n) REFERENCES outputs(txid,n) ON DELETE CASCADE);
CREATE INDEX IF NOT EXISTS asset_notes_asset ON asset_notes(asset);
CREATE TABLE IF NOT EXISTS asset_events(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,operation TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS map_requests(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,destination INTEGER NOT NULL,creator TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS maps(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,block_hash TEXT UNIQUE NOT NULL,height INTEGER UNIQUE NOT NULL,artwork TEXT NOT NULL,image BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS map_reveal_audit(txid TEXT NOT NULL,block_hash TEXT NOT NULL,height INTEGER NOT NULL,PRIMARY KEY(txid,block_hash));
CREATE TABLE IF NOT EXISTS soqumap_requests(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,destination INTEGER NOT NULL,creator TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS soqumap_assignments(txid TEXT PRIMARY KEY REFERENCES soqumap_requests(txid) ON DELETE CASCADE,height INTEGER UNIQUE NOT NULL);
CREATE TABLE IF NOT EXISTS soqumaps(txid TEXT PRIMARY KEY REFERENCES soqumap_assignments(txid) ON DELETE CASCADE,block_hash TEXT UNIQUE NOT NULL,height INTEGER UNIQUE NOT NULL,artwork TEXT NOT NULL,image BLOB NOT NULL);")?;
    Ok(())
}
// Historical metadata commitments from the mainnet index before named creation.
const LEGACY_NAMES: [(&str, &str, &str); 2] = [
    (
        "collection",
        "f5cf0292ab0886c08f58ba0a883231efb5594322c6176e2bbb1927e465f178c1",
        "SOQ.MARKET",
    ),
    (
        "token",
        "68541a3511a2f81582b5a661e174502c7f00d4a6ccd828f4bc2fbf7ed6f093a7",
        "SoqDoge",
    ),
];
const MAINNET_GENESIS: &str = "0d828600816cbd7c23789660b53f90cb6ec7ff85540698e13845eb2d2f0486a8";
const NAMED_CREATION_HEIGHT: u64 = 56_330;

pub(crate) fn seed_legacy_names(db: &Connection) -> Result<()> {
    for (kind, hash, name) in LEGACY_NAMES {
        let catalog = if kind == "token" {
            "assets"
        } else {
            "collections"
        };
        db.execute(
            &format!("INSERT OR IGNORE INTO creation_names SELECT ?1,?2,id FROM {catalog} WHERE metadata=?3
                AND id IN (SELECT txid FROM transactions WHERE height<?5)
                AND EXISTS(SELECT 1 FROM blocks WHERE height=0 AND hash=?4)"),
            params![kind, hex::encode(soq_core::protocol::name_key(name)?), hash, MAINNET_GENESIS, NAMED_CREATION_HEIGHT],
        )?;
    }
    Ok(())
}

pub(crate) fn creation_claim(
    op: &Operation,
) -> Result<Option<(&'static str, Option<String>, Option<String>)>> {
    Ok(match op {
        Operation::Collection { name_key, .. } => {
            Some(("collection", name_key.map(hex::encode), None))
        }
        Operation::Deploy {
            name_key, ticker, ..
        } => {
            let end = ticker.iter().position(|b| *b == 0).unwrap_or(8);
            ensure!(
                end > 0
                    && ticker[..end]
                        .iter()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                    && ticker[end..].iter().all(|b| *b == 0),
                "ticker must be 1 to 8 uppercase alphanumerics"
            );
            Some((
                "token",
                name_key.map(hex::encode),
                Some(std::str::from_utf8(&ticker[..end])?.to_owned()),
            ))
        }
        _ => None,
    })
}

fn index_creation_name(db: &Connection, txid: &str, op: &Operation) -> Result<()> {
    let Some((kind, key, ticker)) = creation_claim(op)? else {
        return Ok(());
    };
    let key = if let Some(key) = key {
        Some(key)
    } else {
        let legacy: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE txid=?1 AND height<?2)
             AND EXISTS(SELECT 1 FROM blocks WHERE height=0 AND hash=?3)",
            params![txid, NAMED_CREATION_HEIGHT, MAINNET_GENESIS],
            |row| row.get(0),
        )?;
        ensure!(legacy, "creation requires an on-chain name commitment");
        let metadata = match op {
            Operation::Collection { metadata, .. } | Operation::Deploy { metadata, .. } => {
                hex::encode(metadata)
            }
            _ => unreachable!(),
        };
        LEGACY_NAMES
            .iter()
            .find(|(k, h, _)| *k == kind && *h == metadata)
            .map(|(_, _, name)| soq_core::protocol::name_key(name).map(hex::encode))
            .transpose()?
    };
    check_confirmed_creation(db, kind, key.as_deref(), ticker.as_deref())?;
    if let Some(key) = key {
        db.execute(
            "INSERT INTO creation_names VALUES(?1,?2,?3)",
            params![kind, key, txid],
        )?;
    }
    Ok(())
}

pub(crate) fn check_confirmed_creation(
    db: &Connection,
    kind: &str,
    key: Option<&str>,
    ticker: Option<&str>,
) -> Result<()> {
    if let Some(key) = key {
        ensure!(
            !db.query_row(
                "SELECT EXISTS(SELECT 1 FROM creation_names WHERE kind=?1 AND name_key=?2)",
                params![kind, key],
                |row| row.get::<_, bool>(0)
            )?,
            "asset name already exists"
        );
    }
    if let Some(ticker) = ticker {
        ensure!(!db.query_row("SELECT EXISTS(SELECT 1 FROM assets WHERE kind='token' AND ticker=?1 COLLATE NOCASE)",
            [ticker], |row| row.get::<_, bool>(0))?, "token ticker already exists");
    }
    Ok(())
}

pub(crate) fn destination(tx: &Transaction<'_>, txid: &str, n: u8) -> Result<()> {
    let (value, script): (u64, String) = tx.query_row(
        "SELECT value,script FROM outputs WHERE txid=?1 AND n=?2",
        params![txid, n],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let script = hex::decode(script)?;
    ensure!(
        value > 0 && script.len() == 34 && script[..2] == [0x51, 32],
        "asset destination must be funded witness v1"
    );
    Ok(())
}
fn note(tx: &Transaction<'_>, txid: &str, n: u8, id: &str, q: u64) -> Result<()> {
    destination(tx, txid, n)?;
    let token_value: Option<u64> = tx
        .query_row(
            "SELECT o.value FROM outputs o JOIN assets a ON a.id=?3 AND a.kind='token'
         WHERE o.txid=?1 AND o.n=?2",
            params![txid, n, id],
            |row| row.get(0),
        )
        .optional()?;
    ensure!(
        token_value.is_none_or(|value| value == soq_core::protocol::TOKEN_OUTPUT_ATOMS),
        "token output must carry exactly 0.01 SOQ"
    );
    ensure!(q > 0 && q <= i64::MAX as u64, "invalid quantity");
    tx.execute(
        "INSERT INTO asset_notes VALUES(?1,?2,?3,?4)",
        params![txid, n, id, q],
    )?;
    Ok(())
}
fn creator(tx: &Transaction<'_>, id: &str, inputs: &[soq_core::OutPoint]) -> Result<String> {
    let input = inputs.first().context("creator input")?;
    let script: String = tx.query_row(
        "SELECT script FROM outputs WHERE txid=?1 AND n=?2 AND spent_by=?3",
        params![input.txid, input.vout, id],
        |r| r.get(0),
    )?;
    let s = hex::decode(&script)?;
    ensure!(
        s.len() == 34 && s[..2] == [0x51, 32],
        "creator input must be witness v1"
    );
    Ok(script)
}
pub(crate) fn generated_mint_creator(
    tx: &Transaction<'_>, txid: &str, inputs: &[soq_core::OutPoint], n: u8,
) -> Result<String> {
    let creator = creator(tx, txid, inputs)?;
    destination(tx, txid, n)?;
    let height: u64 = tx.query_row(
        "SELECT height FROM transactions WHERE txid=?1",
        [txid],
        |row| row.get(0),
    )?;
    for input in inputs {
        let funded: u64 = tx.query_row(
            "SELECT height FROM outputs WHERE txid=?1 AND n=?2",
            params![input.txid, input.vout],
            |row| row.get(0),
        )?;
        ensure!(
            height >= funded + CONFIRMATIONS - 1,
            "generated NFT funding requires six confirmations"
        );
    }
    Ok(creator)
}

pub(crate) fn nft(
    tx: &Transaction<'_>,
    id: &str,
    metadata: &[u8; 32],
    destination: u8,
) -> Result<()> {
    tx.execute(
        "INSERT INTO assets VALUES(?1,'nft',?2,1,1,'')",
        params![id, hex::encode(metadata)],
    )?;
    note(tx, id, destination, id, 1)
}
pub(crate) fn discount_nfts(
    db: &Connection,
    owner: &str,
    next_height: u64,
    include_mempool: bool,
) -> Result<Vec<(&'static str, u8)>> {
    // Pin the first discounted mainnet block so earlier fee decisions replay unchanged.
    if next_height < 56_381
        && db.query_row(
            "SELECT EXISTS(SELECT 1 FROM blocks WHERE height=0 AND hash=?1)",
            [MAINNET_GENESIS],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Ok(vec![]);
    }
    let Some(mature) = next_height.checked_sub(u64::from(CONFIRMATIONS)) else {
        return Ok(vec![]);
    };
    let mut owned = Vec::new();
    let mut statement = db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM asset_notes n JOIN outputs o USING(txid,n)
           WHERE n.asset=?1 AND o.script=?2 AND o.spent_by IS NULL AND o.height<=?3
             AND (NOT ?4 OR NOT EXISTS(SELECT 1 FROM mempool_inputs m WHERE m.previous=o.txid AND m.vout=o.n)))"
    )?;
    for (asset, percent) in soq_core::fees::DISCOUNT_NFTS {
        if statement.query_row(params![asset, owner, mature, include_mempool], |row| {
            row.get::<_, bool>(0)
        })? {
            owned.push((asset, percent));
        }
    }
    Ok(owned)
}

fn input_discount(
    db: &Connection,
    input: Option<&soq_core::OutPoint>,
    height: u64,
    receiver: Option<&[u8]>,
    mempool: bool,
) -> Result<u8> {
    let Some(input) = input else { return Ok(0) };
    let owner: String = db.query_row(
        "SELECT script FROM outputs WHERE txid=?1 AND n=?2",
        params![input.txid, input.vout],
        |row| row.get(0),
    )?;
    if receiver.is_some_and(|script| hex::encode(script) != owner) {
        return Ok(0);
    }
    Ok(discount_nfts(db, &owner, height, mempool)?
        .first()
        .map_or(0, |(_, tier)| *tier))
}

fn validate_platform_fee(outputs: &[Output], base: u64, tier: u8) -> Result<()> {
    let paid = soq_core::fees::carrier_payment(outputs)?;
    ensure!(
        soq_core::fees::authorized_fee(paid, base, tier),
        "invalid platform fee or ineligible NFT discount"
    );
    soq_core::fees::verify_carrier_fee(outputs, paid, None)
}

/// Must be in an SQL savepoint: invalid application data never invalidates an SOQ block.
pub fn apply(
    tx: &Transaction<'_>,
    txid: &str,
    inputs: &[soq_core::OutPoint],
    outputs: &[Output],
    legacy_fees: bool,
    height: u64,
    mempool: bool,
) -> Result<()> {
    let op = soq_core::protocol::operation(outputs)?;
    if let Some(kind) = op
        .platform_kind()
        .filter(|kind| !matches!(kind, soq_core::fees::MintKind::Fatcat))
    {
        if !legacy_fees || matches!(kind, soq_core::fees::MintKind::Listing) {
            if soq_core::fees::verify_fee(outputs, kind).is_err() {
                let tier = input_discount(tx, inputs.first(), height, None, mempool)?;
                validate_platform_fee(outputs, kind.fee(), tier)?;
            }
        } else if !matches!(
            kind,
            soq_core::fees::MintKind::TokenMint | soq_core::fees::MintKind::Collection
        ) {
            let fee = soq_core::fees::fee_output(kind)?;
            let paid = outputs
                .iter()
                .filter(|out| out.script == fee.script)
                .try_fold(0u64, |total, out| {
                    total
                        .checked_add(out.value)
                        .context("platform payment overflow")
                })?;
            ensure!(paid == fee.value, "incorrect platform minting payment");
        }
    }
    index_creation_name(tx, txid, &op)?;
    match &op {
        Operation::FatcatRequest { destination } =>
            super::fatcat::direct_request(tx, txid, inputs, outputs, *destination, mempool)?,
        Operation::FatcatClaim {
            serial,
            destination,
        } => super::fatcat::issue(tx, txid, inputs, *serial, *destination, height)?,
        Operation::FatcatPoolClaim { first, count } => {
            super::fatcat::issue_pool(tx, txid, inputs, outputs, *first, *count, height)?
        }
        Operation::FatcatMint { destination } | Operation::FatcatPoolMint { destination } => {
            super::fatcat::request(
                tx,
                txid,
                inputs,
                outputs,
                *destination,
                height,
                mempool,
                matches!(op, Operation::FatcatPoolMint { .. }),
            )?
        }
        Operation::MapRequest { destination: n } => {
            let creator = creator(tx, txid, inputs)?;
            destination(tx, txid, *n)?;
            tx.execute(
                "INSERT INTO map_requests(txid,destination,creator) VALUES(?1,?2,?3)",
                params![txid, n, creator],
            )?;
        }
        Operation::SoquMapRequest { destination: n } => {
            let creator = generated_mint_creator(tx, txid, inputs, *n)?;
            tx.execute(
                "INSERT INTO soqumap_requests VALUES(?1,?2,?3)",
                params![txid, n, creator],
            )?;
        }
        Operation::Collection {
            metadata, maximum, ..
        } => {
            ensure!(*maximum > 0, "collection maximum must be positive");
            let creator = creator(tx, txid, inputs)?;
            tx.execute(
                "INSERT INTO collections VALUES(?1,?2,?3,?4)",
                params![txid, hex::encode(metadata), creator, maximum],
            )?;
        }
        Operation::CollectionNft {
            collection,
            metadata,
            destination,
        } => {
            let id = hex::encode(collection);
            let (owner, maximum): (String, u64) = tx.query_row(
                "SELECT creator_script,maximum FROM collections WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            ensure!(
                creator(tx, txid, inputs)? == owner,
                "unauthorized collection mint"
            );
            let count: u64 = tx.query_row(
                "SELECT count(*) FROM collection_nfts WHERE collection=?1",
                [&id],
                |r| r.get(0),
            )?;
            ensure!(count < maximum, "collection supply exhausted");
            nft(tx, txid, metadata, *destination)?;
            tx.execute(
                "INSERT INTO collection_nfts VALUES(?1,?2)",
                params![txid, id],
            )?;
        }
        Operation::Nft {
            metadata,
            destination,
        } => nft(tx, txid, metadata, *destination)?,
        Operation::Deploy {
            metadata,
            cap,
            limit,
            ticker,
            ..
        } => {
            ensure!(
                *cap > 0 && *cap <= i64::MAX as u64 && *limit > 0 && limit <= cap,
                "invalid supply parameters"
            );
            let end = ticker.iter().position(|b| *b == 0).unwrap_or(8);
            tx.execute(
                "INSERT INTO assets VALUES(?1,'token',?2,?3,?4,?5)",
                params![
                    txid,
                    hex::encode(metadata),
                    cap,
                    limit,
                    std::str::from_utf8(&ticker[..end])?
                ],
            )?;
        }
        Operation::Mint {
            token,
            amount,
            destination,
        } => {
            let id = hex::encode(token);
            let (cap, limit): (u64, u64) = tx.query_row(
                "SELECT cap,mint_limit FROM assets WHERE id=?1 AND kind='token'",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let minted: u64 = tx.query_row(
                "SELECT coalesce(sum(quantity),0) FROM asset_mints WHERE asset=?1",
                [&id],
                |r| r.get(0),
            )?;
            ensure!(
                *amount > 0
                    && *amount <= limit
                    && minted.checked_add(*amount).is_some_and(|q| q <= cap),
                "mint cap exceeded"
            );
            note(tx, txid, *destination, &id, *amount)?;
            tx.execute(
                "INSERT INTO asset_mints VALUES(?1,?2,?3)",
                params![txid, id, amount],
            )?;
        }
        Operation::Transfer { asset, allocations } => {
            if legacy_fees && soq_core::fees::verify_transfer_fee(outputs).is_err() {
                let carrier = outputs
                    .iter()
                    .position(|out| out.script.first() == Some(&0x6a))
                    .context("transfer carrier missing")?;
                ensure!(
                    outputs.get(carrier + 1) == Some(&soq_core::fees::payment(50_000_000)?),
                    "incorrect historical transfer fee"
                );
            } else {
                let tier = if soq_core::fees::verify_transfer_fee(outputs).is_ok() {
                    0
                } else {
                    input_discount(tx, inputs.first(), height, None, mempool)?
                };
                validate_platform_fee(outputs, soq_core::fees::TRANSFER_FEE, tier)?;
            }
            let id = hex::encode(asset);
            let consumed:u64=tx.query_row("SELECT coalesce(sum(a.quantity),0) FROM asset_notes a JOIN outputs o ON o.txid=a.txid AND o.n=a.n WHERE a.asset=?1 AND o.spent_by=?2",params![id,txid],|r|r.get(0))?;
            let total = allocations.iter().try_fold(0u64, |s, (_, q)| {
                s.checked_add(*q).context("allocation overflow")
            })?;
            ensure!(
                consumed > 0 && consumed == total,
                "unauthorized or unbalanced transfer"
            );
            let mut indices = std::collections::HashSet::new();
            for (n, q) in allocations {
                ensure!(indices.insert(n), "duplicate allocation");
                note(tx, txid, *n, &id, *q)?;
            }
        }
        Operation::Listing { offer } => {
            let payer = inputs.first().context("listing fee input missing")?;
            let script: String = tx.query_row(
                "SELECT script FROM outputs WHERE txid=?1 AND n=?2 AND spent_by=?3",
                params![payer.txid, payer.vout, txid],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO listing_receipts VALUES(?1,?2,?3)",
                params![txid, hex::encode(offer), script],
            )?;
        }
        Operation::Cancel { asset, quantity } => {
            let id = hex::encode(asset);
            let (script, _, old_quantity, _) = full_lot(tx, txid, inputs, &id)?;
            ensure!(old_quantity == *quantity, "cancellation quantity mismatch");
            ensure!(
                outputs
                    .first()
                    .is_some_and(|out| hex::encode(&out.script) == script),
                "cancellation must return the lot to its owner"
            );
            soq_core::fees::verify_exact_payment(outputs, 0, None)?;
            note(tx, txid, 0, &id, *quantity)?;
        }
        Operation::Settle {
            asset,
            quantity,
            price,
        } => {
            let id = hex::encode(asset);
            let (script, anchor, old_quantity, kind) = full_lot(tx, txid, inputs, &id)?;
            ensure!(
                *price > 0 && *quantity == old_quantity,
                "settlement price or quantity mismatch"
            );
            let payout = outputs.first().context("seller payout missing")?;
            let buyer = outputs.get(1).context("buyer output missing")?;
            ensure!(
                hex::encode(&payout.script) == script && buyer.script != payout.script,
                "incorrect seller or buyer script"
            );
            let actual_seller = anchor
                .checked_add(*price)
                .and_then(|total| total.checked_sub(payout.value))
                .context("invalid seller payout")?;
            let base_seller = if kind == "token" {
                soq_core::fees::dex_commission(*price)?
            } else {
                0
            };
            let base_buyer = if kind == "token" {
                base_seller
            } else {
                soq_core::fees::PURCHASE_FEE
            };
            let paid = soq_core::fees::carrier_payment(outputs)?;
            let actual_buyer = paid
                .checked_sub(actual_seller)
                .context("buyer fee missing")?;
            if actual_seller != base_seller {
                let tier = input_discount(tx, inputs.first(), height, None, mempool)?;
                ensure!(
                    soq_core::fees::authorized_fee(actual_seller, base_seller, tier),
                    "seller is ineligible for the signed discount"
                );
            }
            if actual_buyer != base_buyer {
                let tier = input_discount(tx, inputs.get(1), height, Some(&buyer.script), mempool)?;
                ensure!(
                    soq_core::fees::authorized_fee(actual_buyer, base_buyer, tier),
                    "buyer is ineligible for the settlement discount"
                );
            }
            soq_core::fees::verify_carrier_fee(outputs, paid, Some(0))?;
            note(tx, txid, 1, &id, *quantity)?;
        }
    }
    tx.execute(
        "INSERT INTO asset_events VALUES(?1,?2)",
        params![txid, serde_json::to_string(&op)?],
    )?;
    Ok(())
}
fn full_lot(
    tx: &Transaction<'_>,
    txid: &str,
    inputs: &[soq_core::OutPoint],
    asset: &str,
) -> Result<(String, u64, u64, String)> {
    let first = inputs.first().context("asset input missing")?;
    let lot = tx.query_row(
        "SELECT o.script,o.value,n.quantity,a.kind FROM outputs o
         JOIN asset_notes n USING(txid,n) JOIN assets a ON a.id=n.asset
         WHERE o.txid=?1 AND o.n=?2 AND o.spent_by=?3 AND n.asset=?4",
        params![first.txid, first.vout, txid, asset],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let count: u64 = tx.query_row(
        "SELECT count(*) FROM outputs o JOIN asset_notes n USING(txid,n) WHERE o.spent_by=?1",
        [txid],
        |row| row.get(0),
    )?;
    ensure!(count == 1, "requires exactly one asset input");
    Ok(lot)
}
/// Canonical FIFO is (payment block height, transaction position). Each block identity is unique.
/// Six confirmations of payment, followed by six confirmations of the allocated entropy block.
pub fn finalize_maps(tx: &Transaction<'_>, tip: u64) -> Result<()> {
    if tip < 2 * (CONFIRMATIONS - 1) {
        return Ok(());
    }
    loop {
        let mut q=tx.prepare("SELECT r.txid,r.destination,t.height,r.creator FROM map_requests r JOIN transactions t ON t.txid=r.txid LEFT JOIN maps m ON m.txid=r.txid WHERE m.txid IS NULL ORDER BY t.height,t.position LIMIT 1")?;
        let mut rows = q.query([])?;
        let Some(row) = rows.next()? else { break };
        let id: String = row.get(0)?;
        let n: u8 = row.get(1)?;
        let payment_height: u64 = row.get(2)?;
        let next: u64 = tx.query_row("SELECT coalesce(max(height)+1,0) FROM maps", [], |r| {
            r.get(0)
        })?;
        let height = next.max(payment_height + CONFIRMATIONS - 1);
        if height > tip - (CONFIRMATIONS - 1) {
            break;
        }
        let hash: String =
            tx.query_row("SELECT hash FROM blocks WHERE height=?1", [height], |r| {
                r.get(0)
            })?;
        let mut creator_script = [0; 34];
        let creator: String = row.get(3)?;
        hex::decode_to_slice(creator, &mut creator_script)?;
        let (parameters, image) =
            soq_fingerprints::canonical(&creator_script, &id, u32::from(n), &hash, height)?;
        let art = json!({
            "name": format!("{height}.fingerprint"),
            "type": "soq-fingerprint",
            "parameters": parameters,
            "content_sha256": hex::encode(soq_core::sha256(&image)),
        });
        let metadata = hex::encode(soq_core::sha256(&serde_json::to_vec(&art)?));
        tx.execute(
            "INSERT INTO assets VALUES(?1,'fingerprint',?2,1,1,'')",
            params![id, metadata],
        )?;
        note(tx, &id, n, &id, 1)?;
        tx.execute(
            "INSERT INTO maps VALUES(?1,?2,?3,?4,?5)",
            params![id, hash, height, serde_json::to_string(&art)?, image],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO map_reveal_audit VALUES(?1,?2,?3)",
            params![id, hash, height],
        )?;
    }
    Ok(())
}
/// Assign mature payments in canonical transaction order, even when the
/// assigned entropy block is still in the future. Reveal only mature blocks.
pub fn finalize_soqumaps(tx: &Transaction<'_>, tip: u64) -> Result<()> {
    let Some(mature_height) = tip.checked_sub(CONFIRMATIONS - 1) else {
        return Ok(());
    };
    let mut next: u64 = tx.query_row(
        "SELECT coalesce(max(height)+1,0) FROM soqumap_assignments",
        [],
        |r| r.get(0),
    )?;
    let mut pending = tx.prepare(
        "SELECT r.txid,t.height FROM soqumap_requests r JOIN transactions t USING(txid)
         LEFT JOIN soqumap_assignments a USING(txid) WHERE a.txid IS NULL AND t.height<=?1
         ORDER BY t.height,t.position",
    )?;
    let requests = pending
        .query_map([mature_height], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, paid) in requests {
        let height = next.max(paid + CONFIRMATIONS - 1);
        tx.execute(
            "INSERT INTO soqumap_assignments VALUES(?1,?2)",
            params![id, height],
        )?;
        next = height + 1;
    }

    let mut assigned = tx.prepare(
        "SELECT a.txid,a.height,b.hash,r.destination,r.creator FROM soqumap_assignments a
         JOIN soqumap_requests r USING(txid) JOIN blocks b ON b.height=a.height
         LEFT JOIN soqumaps m USING(txid) WHERE m.txid IS NULL AND a.height<=?1 ORDER BY a.height",
    )?;
    let mut rows = assigned.query([mature_height])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let height: u64 = row.get(1)?;
        let hash: String = row.get(2)?;
        let destination: u8 = row.get(3)?;
        let creator: String = row.get(4)?;
        let mut script = [0; 34];
        hex::decode_to_slice(creator, &mut script)?;
        let (art, image) = soq_maps::synthwave::render(&hash, height, &id, &script)?;
        let body = serde_json::to_vec(&art)?;
        let metadata = hex::encode(soq_core::sha256(&body));
        tx.execute(
            "INSERT INTO assets VALUES(?1,'map',?2,1,1,'')",
            params![id, metadata],
        )?;
        note(tx, &id, destination, &id, 1)?;
        tx.execute(
            "INSERT INTO soqumaps VALUES(?1,?2,?3,?4,?5)",
            params![id, hash, height, String::from_utf8(body)?, image],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO map_reveal_audit VALUES(?1,?2,?3)",
            params![id, hash, height],
        )?;
    }
    Ok(())
}

pub fn rollback_maps(tx: &Transaction<'_>, height: u64) -> Result<()> {
    tx.execute(
        "DELETE FROM assets WHERE id IN (SELECT txid FROM soqumaps WHERE height+?1>=?2)",
        params![CONFIRMATIONS - 1, height],
    )?;
    tx.execute(
        "DELETE FROM soqumaps WHERE height+?1>=?2",
        params![CONFIRMATIONS - 1, height],
    )?;
    tx.execute(
        "DELETE FROM soqumap_assignments WHERE txid IN (
            SELECT txid FROM transactions WHERE height+?1>=?2
        )",
        params![CONFIRMATIONS - 1, height],
    )?;
    tx.execute(
        "DELETE FROM assets WHERE id IN (SELECT txid FROM maps WHERE height+?1>=?2)",
        params![CONFIRMATIONS - 1, height],
    )?;
    tx.execute(
        "DELETE FROM maps WHERE height+?1>=?2",
        params![CONFIRMATIONS - 1, height],
    )?;
    Ok(())
}
pub fn catalog(db: &Connection) -> Result<Vec<Value>> {
    let mut q=db.prepare("SELECT a.id,a.kind,a.metadata,a.cap,a.ticker,coalesce((SELECT sum(quantity) FROM asset_mints m WHERE m.asset=a.id),0),n.name_key FROM assets a LEFT JOIN creation_names n ON n.txid=a.id ORDER BY a.id")?;
    let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"metadata_sha256":r.get::<_,String>(2)?,"cap":r.get::<_,u64>(3)?.to_string(),"ticker":r.get::<_,String>(4)?,"minted":r.get::<_,u64>(5)?.to_string()})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn collections(db: &Connection) -> Result<Vec<Value>> {
    let mut q=db.prepare("SELECT c.id,c.metadata,c.maximum,c.creator_script,(SELECT count(*) FROM collection_nfts n WHERE n.collection=c.id),k.name_key FROM collections c LEFT JOIN creation_names k ON k.txid=c.id ORDER BY c.id")?;
    let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"metadata_sha256":r.get::<_,String>(1)?,"maximum":r.get::<_,u64>(2)?,"creator_script":r.get::<_,String>(3)?,"minted":r.get::<_,u64>(4)?,"name_key":r.get::<_,Option<String>>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn balance(db: &Connection, script: &str) -> Result<Vec<Value>> {
    let mut q=db.prepare("SELECT a.asset,sum(a.quantity) FROM asset_notes a JOIN outputs o ON o.txid=a.txid AND o.n=a.n WHERE o.script=?1 AND o.spent_by IS NULL GROUP BY a.asset LIMIT 1000")?;
    let rows = q
        .query_map([script], |r| {
            Ok(json!({"asset":r.get::<_,String>(0)?,"quantity":r.get::<_,u64>(1)?.to_string()}))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub fn has_protocol(outputs: &[Output]) -> bool {
    outputs.iter().any(|o| o.script.first() == Some(&0x6a))
}
