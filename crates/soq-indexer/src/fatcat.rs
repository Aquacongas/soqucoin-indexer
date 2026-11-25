//! Canonical FATCAT requests and an append-only reveal journal.
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use soq_core::{
    protocol::{FATCAT_ATLAS, FATCAT_PRICE, FATCAT_SUPPLY},
    sha256, OutPoint, Output,
};
use std::collections::HashSet;

pub fn schema(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS fatcat_claims(
            serial INTEGER NOT NULL CHECK(serial BETWEEN 1 AND 1006),
            txid TEXT NOT NULL, destination INTEGER NOT NULL, PRIMARY KEY(txid,destination),
            FOREIGN KEY(txid,destination) REFERENCES outputs(txid,n) ON DELETE CASCADE);
         CREATE INDEX IF NOT EXISTS fatcat_claim_serial ON fatcat_claims(serial);
         CREATE TABLE IF NOT EXISTS fatcat_pool_claims(
            txid TEXT NOT NULL,destination INTEGER NOT NULL,
            PRIMARY KEY(txid,destination),
            FOREIGN KEY(txid,destination) REFERENCES fatcat_claims(txid,destination) ON DELETE CASCADE);
         CREATE TABLE IF NOT EXISTS fatcat_requests(
            txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,
            serial INTEGER NOT NULL UNIQUE,
            destination INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS fatcat_draws(
            txid TEXT PRIMARY KEY, serial INTEGER NOT NULL UNIQUE,
            variant INTEGER NOT NULL UNIQUE CHECK(variant BETWEEN 0 AND 1005),
            number INTEGER NOT NULL UNIQUE CHECK(number BETWEEN 1 AND 1006),
            height INTEGER NOT NULL, block_hash TEXT NOT NULL,
            metadata_hash TEXT NOT NULL UNIQUE, metadata BLOB NOT NULL);",
    )?;
    Ok(())
}

pub fn halted(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key='fatcat_halted')",
        [],
        |r| r.get(0),
    )?)
}

pub fn before_rollback(db: &Transaction<'_>, height: u64) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO meta(key,value) SELECT 'fatcat_halted','reveal_reorganized'
         WHERE EXISTS(SELECT 1 FROM fatcat_draws WHERE height>=?1)",
        [height],
    )?;
    db.execute(
        "DELETE FROM assets WHERE id IN (SELECT txid FROM fatcat_draws WHERE height>=?1)",
        [height],
    )?;
    Ok(())
}

pub fn issue(
    db: &Transaction<'_>,
    id: &str,
    inputs: &[OutPoint],
    serial: u16,
    destination: u8,
    height: u64,
) -> Result<()> {
    let first = inputs
        .first()
        .context("FATCAT claim issuer input missing")?;
    let issuer: String = db.query_row(
        "SELECT script FROM outputs WHERE txid=?1 AND n=?2 AND spent_by=?3",
        params![first.txid, first.vout, id],
        |r| r.get(0),
    )?;
    let configured: Option<String> = db
        .query_row(
            "SELECT value FROM meta WHERE key='fatcat_test_issuer'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let expected = configured.unwrap_or(hex::encode(soq_core::fees::payment(0)?.script));
    ensure!(
        issuer == expected,
        "FATCAT claim must be issued by the platform"
    );
    super::assets::destination(db, id, destination)?;
    let unavailable: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fatcat_requests WHERE serial=?1
           UNION SELECT 1 FROM fatcat_draws WHERE serial=?1
           UNION SELECT 1 FROM fatcat_claims c JOIN outputs o ON o.txid=c.txid AND o.n=c.destination
           LEFT JOIN transactions spent ON spent.txid=o.spent_by
           WHERE c.serial=?1 AND (spent.height IS NULL OR spent.height+5>?2))",
        params![serial, height],
        |row| row.get(0),
    )?;
    ensure!(
        !unavailable,
        "FATCAT claim serial is already issued or assigned"
    );

    db.execute(
        "INSERT INTO fatcat_claims VALUES(?1,?2,?3)",
        params![serial, id, destination],
    )?;
    Ok(())
}

pub fn issue_pool(
    db: &Transaction<'_>,
    id: &str,
    inputs: &[OutPoint],
    outputs: &[Output],
    first: u16,
    count: u8,
    height: u64,
) -> Result<()> {
    let n = usize::from(count);
    ensure!(
        matches!(outputs.len(), size if size==n+1 || size==n+2)
            && outputs[n].value == 0
            && outputs[n].script.first() == Some(&0x6a),
        "invalid FATCAT pool issuance output layout"
    );
    let script = &outputs[0].script;
    ensure!(
        *script != soq_core::fees::payment(0)?.script,
        "claim pool must use a dedicated wallet"
    );
    for index in 0..count {
        let out = &outputs[usize::from(index)];
        ensure!(
            out.value == soq_core::protocol::TOKEN_OUTPUT_ATOMS && out.script == *script,
            "FATCAT pool outputs must fund the same signer at 0.01 SOQ each"
        );
        issue(db, id, inputs, first + u16::from(index), index, height)?;
        db.execute(
            "INSERT INTO fatcat_pool_claims VALUES(?1,?2)",
            params![id, index],
        )?;
    }
    Ok(())
}

/// Available artwork/number pairs, net of already indexed payments and optionally mempool payments.
pub fn remaining(db: &Connection, mempool: bool) -> Result<u16> {
    let unavailable: u16 = db.query_row(
        "SELECT count(*) FROM (SELECT number FROM fatcat_draws UNION SELECT variant+1 FROM fatcat_draws)",
        [], |row| row.get(0),
    )?;
    let pending: u16 = db.query_row(
        "SELECT count(*) FROM fatcat_requests r WHERE NOT EXISTS
         (SELECT 1 FROM fatcat_draws d WHERE d.txid=r.txid)",
        [], |row| row.get(0),
    )?;
    let unconfirmed = if mempool {
        let carrier = soq_core::protocol::Operation::FatcatRequest { destination: 0 }.script()?;
        let payment = soq_core::fees::payment(FATCAT_PRICE)?;
        db.query_row(
            "SELECT count(*) FROM mempool_outputs c JOIN mempool_outputs p USING(txid)
             WHERE c.n=1 AND c.value=0 AND c.script=?1 AND p.n=2 AND p.value=?2 AND p.script=?3
               AND NOT EXISTS(SELECT 1 FROM transactions t WHERE t.txid=c.txid)",
            params![hex::encode(carrier), payment.value, hex::encode(payment.script)],
            |row| row.get::<_, u64>(0),
        )?.min(u64::from(FATCAT_SUPPLY)) as u16
    } else { 0 };
    Ok(FATCAT_SUPPLY.saturating_sub(unavailable).saturating_sub(pending).saturating_sub(unconfirmed))
}

pub fn direct_request(
    db: &Transaction<'_>,
    id: &str,
    inputs: &[OutPoint],
    outputs: &[Output],
    destination: u8,
    preflight: bool,
) -> Result<()> {
    ensure!(!halted(db)?, "FATCAT minting suspended after chain reorganization");
    ensure!(
        destination == 0 && matches!(outputs.len(), 3 | 4)
            && outputs[0].value == soq_core::protocol::TOKEN_OUTPUT_ATOMS
            && outputs[1].value == 0 && outputs[1].script.first() == Some(&0x6a)
            && outputs[2] == soq_core::fees::payment(FATCAT_PRICE)?,
        "FATCAT mint requires exactly 2,000 SOQ to the platform"
    );
    let creator = super::assets::generated_mint_creator(db, id, inputs, destination)?;
    ensure!(
        hex::encode(&outputs[0].script) == creator
            && outputs.get(3).is_none_or(|out| out.value > 0 && hex::encode(&out.script) == creator),
        "FATCAT ownership and change must return to the buyer"
    );
    let legacy_pending: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fatcat_requests r WHERE r.serial>0
         AND NOT EXISTS(SELECT 1 FROM fatcat_draws d WHERE d.txid=r.txid))",
        [], |row| row.get(0),
    )?;
    ensure!(!legacy_pending, "Wait for previously paid legacy FATCAT mints to reveal");
    ensure!(remaining(db, preflight)? > 0, "No FATCAT mints remain available");
    // Negative row identifiers distinguish direct payments from historical claim serials.
    // They do not determine the assigned number or reserve any specific artwork.
    db.execute(
        "INSERT INTO fatcat_requests SELECT ?1, min(0, coalesce(min(serial),0))-1, ?2
         FROM (SELECT serial FROM fatcat_requests UNION ALL SELECT serial FROM fatcat_draws)",
        params![id, destination],
    )?;
    Ok(())
}

pub fn request(
    db: &Transaction<'_>,
    id: &str,
    inputs: &[OutPoint],
    outputs: &[Output],
    destination: u8,
    height: u64,
    preflight: bool,
    pooled: bool,
) -> Result<()> {
    let direct_started: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fatcat_requests WHERE serial<0 UNION ALL SELECT 1 FROM fatcat_draws WHERE serial<0)",
        [], |row| row.get(0),
    )?;
    ensure!(!direct_started, "Legacy FATCAT claims are replay-only");
    if preflight {
        ensure!(
            !halted(db)?,
            "FATCAT minting suspended after chain reorganization"
        );
    }
    ensure!(
        destination == 0
            && matches!(outputs.len(), 3 | 4)
            && outputs[1].script.first() == Some(&0x6a)
            && outputs[1].value == 0
            && outputs[2] == soq_core::fees::payment(FATCAT_PRICE)?,
        "FATCAT mint requires exactly 2,000 SOQ to the platform"
    );
    super::assets::destination(db, id, destination)?;
    let first = inputs.first().context("FATCAT claim input missing")?;
    let (serial, claim_height, claim_owner): (u16,u64,String) = db.query_row(
        "SELECT c.serial,o.height,o.script FROM fatcat_claims c JOIN outputs o
         ON o.txid=c.txid AND o.n=c.destination WHERE c.txid=?1 AND c.destination=?2 AND o.spent_by=?3",
        params![first.txid,first.vout,id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?.context("first input must spend an issued FATCAT claim")?;
    let is_pool: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM fatcat_pool_claims WHERE txid=?1 AND destination=?2)",
        params![first.txid, first.vout],
        |r| r.get(0),
    )?;
    ensure!(is_pool == pooled, "wrong FATCAT claim type");
    let owner = if pooled {
        ensure!(inputs.len() >= 2, "FATCAT pool mint requires buyer funding");
        let buyer = hex::encode(&outputs[0].script);
        ensure!(
            buyer != claim_owner,
            "FATCAT pool claims cannot pay for themselves"
        );
        buyer
    } else {
        claim_owner
    };
    ensure!(
        height >= claim_height + 5,
        "FATCAT claim needs six confirmations"
    );
    ensure!(
        outputs
            .get(usize::from(destination))
            .is_some_and(|output| hex::encode(&output.script) == owner
                && output.value == soq_core::protocol::TOKEN_OUTPUT_ATOMS),
        "FATCAT mint must return the NFT anchor to the claim owner"
    );
    ensure!(
        outputs
            .get(3)
            .is_none_or(|out| out.value > 0 && hex::encode(&out.script) == owner),
        "FATCAT mint change must return to the claim owner"
    );
    for input in &inputs[1..] {
        let valid: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM outputs WHERE txid=?1 AND n=?2 AND script=?3)
             AND NOT EXISTS(SELECT 1 FROM fatcat_claims WHERE txid=?1 AND destination=?2
                 UNION SELECT 1 FROM fatcat_requests WHERE txid=?1 AND destination=?2
                 UNION SELECT 1 FROM asset_notes WHERE txid=?1 AND n=?2
                 UNION SELECT 1 FROM map_requests WHERE txid=?1 AND destination=?2
                 UNION SELECT 1 FROM soqumap_requests WHERE txid=?1 AND destination=?2)
             AND EXISTS(SELECT 1 FROM outputs WHERE txid=?1 AND n=?2 AND coinbase=0 AND height+5<=?4)",
            params![input.txid, input.vout, owner,height],
            |r| r.get(0),
        )?;
        ensure!(
            valid,
            "FATCAT funding must belong to the claim owner and spend exactly one claim"
        );
    }
    let prior: Option<String> = db
        .query_row(
            "SELECT txid FROM fatcat_draws WHERE serial=?1",
            [serial],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        prior.is_none_or(|txid| txid == id),
        "FATCAT claim already assigned"
    );
    db.execute(
        "INSERT INTO fatcat_requests VALUES(?1,?2,?3)",
        params![id, serial, destination],
    )?;
    Ok(())
}

fn draw_index(domain: &[u8], hash: &[u8; 32], id: &[u8; 32], bound: usize) -> usize {
    let bound = bound as u64;
    let threshold = bound.wrapping_neg() % bound;
    let mut seed = Vec::from(b"SOQ.MARKET FATCAT draw v1\0".as_slice());
    seed.extend_from_slice(domain);
    seed.extend_from_slice(hash);
    seed.extend_from_slice(id);
    let prefix = seed.len();
    for counter in 0u64.. {
        seed.truncate(prefix);
        seed.extend_from_slice(&counter.to_le_bytes());
        let draw = u64::from_le_bytes(sha256(&seed)[..8].try_into().unwrap());
        if draw >= threshold {
            return (draw % bound) as usize;
        }
    }
    unreachable!()
}

pub fn finalize(db: &Transaction<'_>, height: u64, hash: &str) -> Result<()> {
    let Some(eligible) = height.checked_sub(5) else {
        return Ok(());
    };
    if halted(db)? {
        return Ok(());
    }
    let mut q = db.prepare(
        "SELECT r.txid,r.serial,r.destination FROM fatcat_requests r JOIN transactions t USING(txid)
         LEFT JOIN fatcat_draws d USING(txid) WHERE t.height=?1 AND d.txid IS NULL ORDER BY t.position",
    )?;
    let requests = q
        .query_map([eligible], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, u8>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if requests.is_empty() {
        return Ok(());
    }
    let mut q = db.prepare("SELECT variant,number FROM fatcat_draws")?;
    let used = q
        .query_map([], |r| Ok((r.get::<_, u16>(0)?, r.get::<_, u16>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let variants_used: HashSet<_> = used.iter().map(|pair| pair.0).collect();
    let numbers_used: HashSet<_> = used.iter().map(|pair| pair.1).collect();
    let mut variants: Vec<_> = (0..FATCAT_SUPPLY)
        .filter(|x| !variants_used.contains(x))
        .collect();
    let mut numbers: Vec<_> = (1..=FATCAT_SUPPLY)
        .filter(|x| !numbers_used.contains(x))
        .collect();
    let mut paired: Vec<_> = numbers.iter().copied()
        .filter(|number| !variants_used.contains(&(number - 1))).collect();
    let entropy = soq_core::hash_bytes(hash)?;
    for (id, serial, destination) in requests {
        let txid = soq_core::hash_bytes(&id)?;
        let (variant, number) = if serial < 0 {
            ensure!(!paired.is_empty(), "FATCAT assignments would exceed supply");
            let number = paired.remove(draw_index(b"paired-number\0", &entropy, &txid, paired.len()));
            (number - 1, number)
        } else {
            ensure!(!variants.is_empty() && !numbers.is_empty(), "FATCAT assignments would exceed supply");
            let variant = variants.remove(draw_index(b"variant\0", &entropy, &txid, variants.len()));
            let number = numbers.remove(draw_index(b"number\0", &entropy, &txid, numbers.len()));
            (variant, number)
        };
        let mut metadata = json!({
            "name":format!("FATCAT#{number}"),"symbol":"FATCAT","fatcat_variant":variant,
            "fatcat_number":number,"atlas_rgb_sha256":FATCAT_ATLAS,"reveal_height":height,
            "reveal_block":hash,"mint_txid":id,"draw_version":if serial < 0 { 2 } else { 1 }
        });
        if serial > 0 {
            metadata["claim_serial"] = json!(serial);
        }
        let metadata = serde_json::to_vec(&metadata)?;
        let commitment = sha256(&metadata);
        db.execute(
            "INSERT INTO fatcat_draws VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                serial,
                variant,
                number,
                height,
                hash,
                hex::encode(commitment),
                metadata
            ],
        )?;
        super::assets::nft(db, &id, &commitment, destination)?;
    }
    Ok(())
}

fn legacy_serial(serial: i64) -> Option<u16> {
    u16::try_from(serial).ok()
}

pub fn status(db: &Connection, id: &str) -> Result<Option<Value>> {
    let request = db
        .query_row(
            "SELECT r.serial,d.number,d.variant,d.height,d.block_hash,
          EXISTS(SELECT 1 FROM blocks b WHERE b.height=d.height AND b.hash=d.block_hash)
            AND EXISTS(SELECT 1 FROM assets WHERE id=r.txid)
         FROM fatcat_requests r LEFT JOIN fatcat_draws d USING(txid) WHERE r.txid=?1",
            [id],
            |r| {
                Ok(
                    json!({"claim":legacy_serial(r.get(0)?),"number":r.get::<_,Option<u16>>(1)?,
            "variant":r.get::<_,Option<u16>>(2)?,"reveal_height":r.get::<_,Option<u64>>(3)?,
            "reveal_block":r.get::<_,Option<String>>(4)?,"canonical_reveal":r.get::<_,bool>(5)?}),
                )
            },
        )
        .optional()?;
    if request.is_some() {
        return Ok(request);
    }
    db.query_row(
        "SELECT serial,number,variant,height,block_hash FROM fatcat_draws WHERE txid=?1",
        [id],
        |r| {
            Ok(
                json!({"claim":legacy_serial(r.get(0)?),"number":r.get::<_,u16>(1)?,
         "variant":r.get::<_,u16>(2)?,"reveal_height":r.get::<_,u64>(3)?,
         "reveal_block":r.get::<_,String>(4)?,"canonical_reveal":false,"orphaned":true}),
            )
        },
    )
    .optional()
    .map_err(Into::into)
}
