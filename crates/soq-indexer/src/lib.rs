mod fatcat;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use soq_core::{atoms, hash_bytes, OutPoint};
use std::{path::Path, time::Duration};
pub mod assets;

mod rpc;
pub use rpc::Rpc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coin {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub script: String,
    pub height: u64,
    pub coinbase: bool,
}
pub struct Store {
    db: Connection,
}
pub struct ListingTerms {
    pub quantity: u64,
    pub price: u64,
}
fn required_str<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().with_context(|| format!("missing {key}"))
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
 CREATE TABLE IF NOT EXISTS blocks(height INTEGER PRIMARY KEY,hash TEXT UNIQUE NOT NULL,parent TEXT NOT NULL,time INTEGER NOT NULL);
 CREATE TABLE IF NOT EXISTS transactions(txid TEXT PRIMARY KEY,height INTEGER NOT NULL REFERENCES blocks(height) ON DELETE CASCADE,position INTEGER NOT NULL);
 CREATE TABLE IF NOT EXISTS outputs(txid TEXT NOT NULL REFERENCES transactions(txid) ON DELETE CASCADE,n INTEGER NOT NULL,value INTEGER NOT NULL,script TEXT NOT NULL,height INTEGER NOT NULL,coinbase INTEGER NOT NULL,spent_by TEXT REFERENCES transactions(txid) ON DELETE SET NULL,PRIMARY KEY(txid,n));
 CREATE INDEX IF NOT EXISTS outputs_script_unspent ON outputs(script,spent_by);
 CREATE INDEX IF NOT EXISTS tx_height ON transactions(height,position);
 CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS pending(txid TEXT PRIMARY KEY,raw TEXT NOT NULL,state TEXT NOT NULL);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS mempool(txid TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS invalid_asset_operations(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,reason TEXT NOT NULL);")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS observed_mempool(txid TEXT PRIMARY KEY REFERENCES mempool(txid) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS mempool_outputs(txid TEXT REFERENCES observed_mempool(txid) ON DELETE CASCADE,n INTEGER,value INTEGER NOT NULL,script TEXT NOT NULL,protected INTEGER NOT NULL,PRIMARY KEY(txid,n));
        CREATE INDEX IF NOT EXISTS mempool_outputs_script ON mempool_outputs(script);
        CREATE TABLE IF NOT EXISTS mempool_inputs(txid TEXT REFERENCES observed_mempool(txid) ON DELETE CASCADE,n INTEGER,previous TEXT NOT NULL,vout INTEGER NOT NULL,PRIMARY KEY(txid,n));
        CREATE INDEX IF NOT EXISTS mempool_inputs_outpoint ON mempool_inputs(previous,vout);")?;
        let version: Option<String> = db
            .query_row(
                "SELECT value FROM meta WHERE key='ledger_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        const VERSION: &str = "sqr20-1-soq721-1-fingerprint-1-fees-1-compact-1";
        if let Some(version) = version {
            ensure!(
                version == VERSION,
                "ledger version differs; rebuild the index from the node"
            );
        } else {
            let count: u64 = db.query_row("SELECT count(*) FROM blocks", [], |r| r.get(0))?;
            ensure!(count == 0, "unversioned existing index requires rebuilding");
            db.execute("INSERT INTO meta VALUES('ledger_version',?1)", [VERSION])?;
        }
        db.execute_batch("CREATE INDEX IF NOT EXISTS outputs_spent_by ON outputs(spent_by) WHERE spent_by IS NOT NULL;")?;
        assets::schema(&db)?;
        fatcat::schema(&db)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS listings(id TEXT PRIMARY KEY,txid TEXT NOT NULL,n INTEGER NOT NULL,asset TEXT NOT NULL,quantity INTEGER NOT NULL,price INTEGER NOT NULL,payout INTEGER NOT NULL,seller_script TEXT NOT NULL,offer_json TEXT NOT NULL,UNIQUE(txid,n),FOREIGN KEY(txid,n) REFERENCES outputs(txid,n) ON DELETE CASCADE);
        CREATE TABLE IF NOT EXISTS trades(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,listing TEXT NOT NULL REFERENCES listings(id) ON DELETE CASCADE,price INTEGER NOT NULL,quantity INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS listings_asset ON listings(asset);
        CREATE INDEX IF NOT EXISTS trades_listing ON trades(listing);")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS listing_receipts(txid TEXT PRIMARY KEY REFERENCES transactions(txid) ON DELETE CASCADE,offer TEXT NOT NULL,payer TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS listing_receipts_offer ON listing_receipts(offer,payer);
             CREATE TABLE IF NOT EXISTS listing_submissions(offer TEXT NOT NULL,payer TEXT NOT NULL,txid TEXT NOT NULL REFERENCES pending(txid),PRIMARY KEY(offer,payer));"
        )?;
        Self::upgrade_integer_tokens(&db)?;
        assets::seed_legacy_names(&db)?;
        Ok(Self { db })
    }
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-4096")?;
        Ok(Self { db })
    }

    pub fn snapshot<T>(&self, read: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let transaction = self.db.unchecked_transaction()?;
        let result = read(self)?;
        transaction.rollback()?;
        Ok(result)
    }

    fn upgrade_integer_tokens(db: &Connection) -> Result<()> {
        if !db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('assets') WHERE name='decimals')",
            [],
            |row| row.get::<_, bool>(0),
        )? {
            return Ok(());
        }
        let t = db.unchecked_transaction()?;
        t.execute_batch(
            "DELETE FROM asset_events WHERE txid IN (
                SELECT id FROM assets WHERE kind='token'
                UNION SELECT m.txid FROM asset_mints m JOIN assets a ON a.id=m.asset WHERE a.kind='token'
                UNION SELECT n.txid FROM asset_notes n JOIN assets a ON a.id=n.asset WHERE a.kind='token'
             );
             DELETE FROM listings WHERE asset IN (SELECT id FROM assets WHERE kind='token');
             DELETE FROM assets WHERE kind='token';
             ALTER TABLE assets DROP COLUMN decimals;",
        )?;
        t.commit()?;
        Ok(())
    }
    pub fn tip(&self) -> Result<Option<(u64, String)>> {
        Self::read_tip(&self.db)
    }
    fn read_tip(db: &Connection) -> Result<Option<(u64, String)>> {
        Ok(db
            .query_row(
                "SELECT height,hash FROM blocks ORDER BY height DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    pub fn bind_chain(&self, genesis: &str) -> Result<()> {
        hash_bytes(genesis)?;
        let old: Option<String> = self
            .db
            .query_row("SELECT value FROM meta WHERE key='genesis'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(old) = old {
            ensure!(old == genesis, "database belongs to another chain");
        } else {
            self.db
                .execute("INSERT INTO meta VALUES('genesis',?1)", [genesis])?;
        }
        Ok(())
    }
    pub fn rollback(&mut self, height: u64) -> Result<()> {
        let t = self.db.transaction()?;
        fatcat::before_rollback(&t, height)?;
        assets::rollback_maps(&t, height)?;
        t.execute("DELETE FROM mempool", [])?;
        t.execute("UPDATE pending SET state='reorganized' WHERE txid IN (SELECT txid FROM transactions WHERE height>=?1)",[height])?;
        t.execute("DELETE FROM transactions WHERE height>=?1", [height])?;
        t.execute("DELETE FROM blocks WHERE height>=?1", [height])?;
        t.commit()?;
        Ok(())
    }
    pub fn apply(&mut self, block: &Value) -> Result<()> {
        self.apply_window(std::slice::from_ref(block))
    }
    pub fn apply_window(&mut self, blocks: &[Value]) -> Result<()> {
        let transaction = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut tip = Self::read_tip(&transaction)?;
        for block in blocks {
            tip = Self::apply_block(&transaction, block, tip)?;
        }
        transaction.commit()?;
        Ok(())
    }
    fn apply_block(
        t: &rusqlite::Transaction<'_>,
        b: &Value,
        tip: Option<(u64, String)>,
    ) -> Result<Option<(u64, String)>> {
        let height = b["height"].as_u64().context("height")?;
        let hash = required_str(b, "hash")?;
        hash_bytes(hash)?;
        let parent = b["previousblockhash"].as_str().unwrap_or("");
        if let Some((h, tip_hash)) = &tip {
            if *h == height && tip_hash == hash {
                return Ok(tip);
            }
            ensure!(
                height == h + 1 && parent == tip_hash,
                "block does not extend canonical tip"
            );
        } else {
            ensure!(height == 0 && parent.is_empty(), "must start at genesis");
            let bound: Option<String> = t
                .query_row("SELECT value FROM meta WHERE key='genesis'", [], |row| {
                    row.get(0)
                })
                .optional()?;
            ensure!(
                bound.is_none_or(|genesis| genesis == hash),
                "block differs from the bound genesis"
            );
        }
        let transactions = b["tx"]
            .as_array()
            .context("getblock verbosity 2 required")?;
        ensure!(
            !transactions.is_empty() && transactions.len() <= 100_000,
            "invalid transaction count"
        );
        t.execute(
            "INSERT INTO blocks VALUES(?1,?2,?3,?4)",
            params![height, hash, parent, b["time"].as_u64().context("time")?],
        )?;
        let legacy_fees = height < 56_250
            && t.query_row(
                "SELECT EXISTS(SELECT 1 FROM blocks WHERE height=0 AND hash=?1)",
                ["0d828600816cbd7c23789660b53f90cb6ec7ff85540698e13845eb2d2f0486a8"],
                |row| row.get::<_, bool>(0),
            )?;
        for (position, tx) in transactions.iter().enumerate() {
            let txid = required_str(tx, "txid")?;
            t.prepare_cached("DELETE FROM mempool WHERE txid=?1")?
                .execute([txid])?;
            t.prepare_cached("UPDATE pending SET state='confirmed' WHERE txid=?1")?
                .execute([txid])?;
            hash_bytes(txid)?;
            t.prepare_cached("INSERT INTO transactions VALUES(?1,?2,?3)")?
                .execute(params![txid, height, position])?;
            let inputs = tx["vin"].as_array().context("vin")?;
            ensure!(!inputs.is_empty(), "empty vin");
            let coinbase = position == 0;
            ensure!(
                coinbase == inputs[0].get("coinbase").is_some(),
                "coinbase position mismatch"
            );
            let mut prevouts = Vec::new();
            if !coinbase {
                for input in inputs {
                    let txid = required_str(input, "txid")?.to_owned();
                    let vout = input["vout"].as_u64().context("vout")?.try_into()?;
                    prevouts.push(OutPoint { txid, vout });
                }
                let mut spend = t.prepare_cached(
                    "UPDATE outputs SET spent_by=?1 WHERE txid=?2 AND n=?3 AND spent_by IS NULL",
                )?;
                for input in &prevouts {
                    ensure!(
                        spend.execute(params![txid, input.txid, input.vout])? == 1,
                        "missing or already spent input"
                    );
                }
            }
            let mut asset_outputs = Vec::new();
            let mut insert_output =
                t.prepare_cached("INSERT INTO outputs VALUES(?1,?2,?3,?4,?5,?6,NULL)")?;
            for (n, o) in tx["vout"].as_array().context("vout")?.iter().enumerate() {
                ensure!(
                    o["n"].as_u64() == Some(n as u64),
                    "noncanonical output index"
                );
                let value = atoms(&o["value"].to_string())?;
                let script = required_str(&o["scriptPubKey"], "hex")?;
                asset_outputs.push(soq_core::Output {
                    value,
                    script: hex::decode(script)?,
                });
                insert_output.execute(params![txid, n, value, script, height, coinbase])?;
            }
            if !coinbase && assets::has_protocol(&asset_outputs) {
                t.execute_batch("SAVEPOINT asset_operation")?;
                if let Err(error) = assets::apply(
                    t,
                    txid,
                    &prevouts,
                    &asset_outputs,
                    legacy_fees,
                    height,
                    false,
                ) {
                    t.execute_batch("ROLLBACK TO asset_operation")?;
                    t.execute(
                        "INSERT INTO invalid_asset_operations VALUES(?1,?2)",
                        params![txid, error.to_string()],
                    )?;
                }
                t.execute_batch("RELEASE asset_operation")?;
            }
            let settlement = match soq_core::protocol::operation(&asset_outputs) {
                Ok(assets::Operation::Settle {
                    asset,
                    quantity,
                    price,
                }) => Some((asset, quantity, Some(price))),
                Ok(assets::Operation::Transfer { asset, allocations })
                    if legacy_fees && allocations.len() == 1 && allocations[0].0 == 1 =>
                {
                    Some((asset, allocations[0].1, None))
                }
                _ => None,
            };
            if let (Some((asset, quantity, price)), Some(first), Some(payout)) =
                (settlement, prevouts.first(), asset_outputs.first())
            {
                t.execute(
                    "INSERT INTO trades SELECT ?1,l.id,l.price,l.quantity FROM listings l
                     WHERE l.txid=?2 AND l.n=?3 AND l.asset=?4 AND l.quantity=?5
                       AND l.payout=?6 AND l.seller_script=?7 AND (?8 IS NULL OR l.price=?8)
                       AND EXISTS(SELECT 1 FROM asset_notes a WHERE a.txid=?1 AND a.n=1 AND a.asset=l.asset AND a.quantity=l.quantity)",
                    params![txid,first.txid,first.vout,hex::encode(asset),quantity,payout.value,hex::encode(&payout.script),price],
                )?;
            }
        }
        assets::finalize_maps(t, height)?;
        assets::finalize_soqumaps(t, height)?;
        fatcat::finalize(t, height, hash)?;
        Ok(Some((height, hash.to_owned())))
    }
    pub fn coins(&self, script: &str) -> Result<Vec<Coin>> {
        let mut q=self.db.prepare("SELECT txid,n,value,script,height,coinbase FROM outputs WHERE script=?1 AND spent_by IS NULL AND height>0 ORDER BY height,txid,n LIMIT 10001")?;
        let coins = q
            .query_map([script], |r| {
                Ok(Coin {
                    txid: r.get(0)?,
                    vout: r.get(1)?,
                    value: r.get(2)?,
                    script: r.get(3)?,
                    height: r.get(4)?,
                    coinbase: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            coins.len() <= 10000,
            "address exceeds UTXO limit; pagination required"
        );
        Ok(coins)
    }
    pub fn assets(&self) -> Result<Vec<Value>> {
        let mut rows = assets::catalog(&self.db)?;
        for row in &mut rows {
            row["standard"] = json!(if row["kind"] == "token" {
                "SQR-20"
            } else {
                "SOQ-721"
            });
            let id = row["id"].as_str().context("asset id")?;
            let (collection, virtual_collection) = self.nft_collections(id)?;
            row["collection"] = json!(collection);
            row["virtual_collection"] = json!(virtual_collection);
        }
        Ok(rows)
    }
    fn nft_collections(&self, id: &str) -> Result<(Option<String>, Option<String>)> {
        Ok(self.db.query_row(
            "SELECT (SELECT collection FROM collection_nfts WHERE txid=?1),
                    CASE WHEN EXISTS(SELECT 1 FROM fatcat_draws WHERE txid=?1) THEN 'fatcats' END",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub fn collections(&self) -> Result<Vec<Value>> {
        let mut rows = assets::collections(&self.db)?;
        for r in &mut rows {
            r["standard"] = json!("SOQ-721");
        }
        Ok(rows)
    }
    pub fn asset(&self, id: &str) -> Result<Value> {
        soq_core::hash_bytes(id)?;
        let mut value=self.db.query_row("SELECT kind,metadata,cap,mint_limit,ticker FROM assets WHERE id=?1",[id],|r|Ok(json!({"id":id,"kind":r.get::<_,String>(0)?,"metadata_sha256":r.get::<_,String>(1)?,"cap":r.get::<_,u64>(2)?.to_string(),"per_mint_limit":r.get::<_,u64>(3)?.to_string(),"ticker":r.get::<_,String>(4)?})))?;
        value["name_key"] = json!(self
            .db
            .query_row(
                "SELECT name_key FROM creation_names WHERE txid=?1",
                [id],
                |row| row.get::<_, String>(0)
            )
            .optional()?);
        value["standard"] = json!(if value["kind"] == "token" {
            "SQR-20"
        } else {
            "SOQ-721"
        });
        if value["kind"] == "nft" {
            let (collection, virtual_collection) = self.nft_collections(id)?;
            value["collection"] = json!(collection);
            value["virtual_collection"] = json!(virtual_collection);
        }
        let (supply,count):(u64,u64)=self.db.query_row("SELECT coalesce(sum(n.quantity),0),count(DISTINCT o.script) FROM asset_notes n JOIN outputs o ON o.txid=n.txid AND o.n=n.n WHERE n.asset=?1 AND o.spent_by IS NULL",[id],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let minted: u64 = self.db.query_row(
            "SELECT coalesce(sum(quantity),0) FROM asset_mints WHERE asset=?1",
            [id],
            |r| r.get(0),
        )?;
        value["circulating_units"] = json!(supply.to_string());
        value["holders"] = json!(count);
        value["minted_units"] = json!(minted.to_string());
        Ok(value)
    }
    /// Read the script of a particular prevout spent by the given indexed transaction.
    pub fn spent_output_script(
        &self,
        txid: &str,
        vout: u32,
        spent_by: &str,
    ) -> Result<Option<Vec<u8>>> {
        let script: Option<String> = self
            .db
            .query_row(
                "SELECT script FROM outputs WHERE txid=?1 AND n=?2 AND spent_by=?3",
                params![txid, vout, spent_by],
                |row| row.get(0),
            )
            .optional()?;
        Ok(script.map(hex::decode).transpose()?)
    }

    pub fn holders(&self, id: &str, hrp: &str) -> Result<Vec<Value>> {
        soq_core::hash_bytes(id)?;
        let mut q=self.db.prepare("SELECT o.script,sum(n.quantity) AS quantity FROM asset_notes n JOIN outputs o ON o.txid=n.txid AND o.n=n.n WHERE n.asset=?1 AND o.spent_by IS NULL GROUP BY o.script ORDER BY quantity DESC,o.script LIMIT 1000")?;
        let rows = q
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|(s,q)|Ok(json!({"address":soq_core::script_address(&hex::decode(s)?,hrp)?,"quantity":q.to_string()}))).collect()
    }
    pub fn map_artwork(&self, height: u64) -> Result<Value> {
        self.generated_artwork(height, false)
    }
    pub fn generated_artwork(&self, height: u64, soqumap: bool) -> Result<Value> {
        let table = if soqumap { "soqumaps" } else { "maps" };
        let (origin, body): (String, String) = self.db.query_row(
            &format!("SELECT txid,artwork FROM {table} WHERE height=?1"),
            [height],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut value: Value = serde_json::from_str(&body)?;
        if !soqumap {
            value["name"] = json!(format!("{height}.fingerprint"));
        }
        value["asset_origin_txid"] = json!(origin);
        value["identity_height"] = json!(height);
        let mut q=self.db.prepare(&format!("SELECT a.block_hash FROM map_reveal_audit a JOIN {table} m ON m.txid=a.txid WHERE m.height=?1 AND a.block_hash!=m.block_hash ORDER BY a.block_hash"))?;
        let prior = q
            .query_map([height], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        value["source_reorganized"] = json!(!prior.is_empty());
        value["prior_block_hashes"] = json!(prior);
        Ok(value)
    }
    pub fn maps(&self) -> Result<Vec<Value>> {
        self.generated_artworks(false)
    }
    pub fn generated_artworks(&self, soqumap: bool) -> Result<Vec<Value>> {
        let table = if soqumap { "soqumaps" } else { "maps" };
        let mut q = self
            .db
            .prepare(&format!("SELECT height FROM {table} ORDER BY height DESC"))?;
        let rows = q
            .query_map([], |r| r.get::<_, u64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|height| self.generated_artwork(height, soqumap))
            .collect()
    }
    pub fn map_content(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes: Vec<u8> = self.db.query_row(
            "SELECT CASE WHEN a.metadata=?1 THEN CAST(m.artwork AS BLOB) ELSE m.image END
             FROM (SELECT txid,artwork,image FROM maps UNION ALL SELECT txid,artwork,image FROM soqumaps) m JOIN assets a ON a.id=m.txid
             WHERE a.metadata=?1 OR json_extract(m.artwork,'$.content_sha256')=?1 LIMIT 1",
            [hash],
            |r| r.get(0),
        )?;
        ensure!(
            hex::encode(soq_core::sha256(&bytes)) == hash,
            "generated artwork content integrity failure"
        );
        Ok(bytes)
    }
    pub fn artwork_image_hashes(&self) -> Result<Vec<String>> {
        let mut query = self.db.prepare_cached(
            "SELECT json_extract(artwork,'$.content_sha256') FROM maps
             UNION SELECT json_extract(artwork,'$.content_sha256') FROM soqumaps",
        )?;
        let rows = query.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn transaction_location(&self, id: &str) -> Result<(u64, String)> {
        hash_bytes(id)?;
        Ok(self.db.query_row(
            "SELECT t.height,b.hash FROM transactions t JOIN blocks b USING(height) WHERE t.txid=?1",
            [id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }
    fn summary(&self, id: &str, height: u64) -> Result<Value> {
        // Preserve the activity label derived from the first matching output.
        let script: Option<String> = self
            .db
            .query_row(
                "SELECT substr(script,instr(script,'53514d4b01')+10,2) FROM outputs
             WHERE txid=?1 AND instr(script,'53514d4b01')>0 ORDER BY n LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let operation = script.and_then(|hex| u8::from_str_radix(&hex, 16).ok());
        Ok(json!({"txid":id,"height":height,"operation_type":operation}))
    }
    pub fn block(&self, height: u64) -> Result<Value> {
        let (hash, parent, time): (String, String, u64) = self.db.query_row(
            "SELECT hash,parent,time FROM blocks WHERE height=?1",
            [height],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut q = self.db.prepare(
            "SELECT txid FROM transactions WHERE height=?1 ORDER BY position LIMIT 1000",
        )?;
        let ids = q
            .query_map([height], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"height":height,"hash":hash,"parent":parent,"time":time,"transactions":ids,"transaction_limit":1000}),
        )
    }
    pub fn history(&self, script: &str) -> Result<Vec<Value>> {
        let mut result = self.mempool_history(script)?;
        result.extend(self.confirmed_history(script)?);
        result.truncate(100);
        Ok(result)
    }

    pub fn confirmed_history(&self, script: &str) -> Result<Vec<Value>> {
        let mut query = self.db.prepare_cached(
            "WITH recent AS MATERIALIZED (
                SELECT txid,height,position FROM transactions WHERE txid IN (
                    SELECT txid FROM outputs WHERE script=?1
                    UNION SELECT spent_by FROM outputs WHERE script=?1 AND spent_by IS NOT NULL
                ) ORDER BY height DESC,position DESC LIMIT 100
             )
             SELECT txid,height,
                (SELECT substr(o.script,instr(o.script,'53514d4b01')+10,2) FROM outputs o
                 WHERE o.txid=r.txid AND instr(o.script,'53514d4b01')>0 ORDER BY o.n LIMIT 1),
                (SELECT coalesce(sum(value),0) FROM outputs o INDEXED BY sqlite_autoindex_outputs_1
                 WHERE o.txid=r.txid AND o.script=?1),
                (SELECT coalesce(sum(value),0) FROM outputs o INDEXED BY outputs_spent_by
                 WHERE o.spent_by=r.txid AND o.script=?1),
                (SELECT l.seller_script=?1 FROM trades t JOIN listings l ON l.id=t.listing WHERE t.txid=r.txid)
             FROM recent r ORDER BY height DESC,position DESC"
        )?;
        let tip = self.tip()?;
        let mut rows = query.query([script])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let height: u64 = row.get(1)?;
            let marker: Option<String> = row.get(2)?;
            let operation = marker.and_then(|hex| u8::from_str_radix(&hex, 16).ok());
            let sale: Option<bool> = row.get(5)?;
            result.push(json!({
                "txid":row.get::<_,String>(0)?, "height":height, "operation_type":operation,
                "marketplace_action":sale.map(|seller| if seller { "Marketplace sale" } else { "Marketplace purchase" }),
                "received_atoms":row.get::<_,u64>(3)?.to_string(), "spent_atoms":row.get::<_,u64>(4)?.to_string(),
                "confirmations":tip.as_ref().map(|(tip,_)| tip.saturating_sub(height)+1).unwrap_or(0),
            }));
        }
        Ok(result)
    }

    pub fn mempool_history(&self, script: &str) -> Result<Vec<Value>> {
        let mut query = self.db.prepare_cached(
            "WITH activity AS (
                SELECT txid FROM mempool_outputs WHERE script=?1
                UNION SELECT i.txid FROM outputs o JOIN mempool_inputs i
                    ON i.previous=o.txid AND i.vout=o.n WHERE o.script=?1
                UNION SELECT i.txid FROM mempool_outputs o JOIN mempool_inputs i
                    ON i.previous=o.txid AND i.vout=o.n WHERE o.script=?1
             )
             SELECT a.txid,
                (SELECT substr(o.script,instr(o.script,'53514d4b01')+10,2) FROM mempool_outputs o
                 WHERE o.txid=a.txid AND instr(o.script,'53514d4b01')>0 ORDER BY o.n LIMIT 1),
                (SELECT coalesce(sum(o.value),0) FROM mempool_outputs o WHERE o.txid=a.txid AND o.script=?1),
                (SELECT coalesce(sum(coalesce(o.value,p.value)),0) FROM mempool_inputs i
                 LEFT JOIN outputs o ON o.txid=i.previous AND o.n=i.vout
                 LEFT JOIN mempool_outputs p ON p.txid=i.previous AND p.n=i.vout
                 WHERE i.txid=a.txid AND coalesce(o.script,p.script)=?1)
             FROM activity a WHERE NOT EXISTS(SELECT 1 FROM transactions t WHERE t.txid=a.txid)
             ORDER BY a.txid LIMIT 100",
        )?;
        let rows = query
            .query_map([script], |row| {
                let marker: Option<String> = row.get(1)?;
                Ok(json!({
                    "txid":row.get::<_,String>(0)?,"confirmations":0,
                    "operation_type":marker.and_then(|hex| u8::from_str_radix(&hex, 16).ok()),
                    "received_atoms":row.get::<_,u64>(2)?.to_string(),
                    "spent_atoms":row.get::<_,u64>(3)?.to_string(),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn wallet_funding(&self, script: &str, network: &str) -> Result<Value> {
        let tip = self.tip()?;
        let mut query = self.db.prepare_cached(
            "SELECT o.txid,o.n,o.value,o.height,o.coinbase,a.asset,a.quantity,
                 (SELECT m.txid FROM mempool_inputs m WHERE m.previous=o.txid AND m.vout=o.n LIMIT 1),
                 a.asset IS NOT NULL
                    OR EXISTS(SELECT 1 FROM map_requests r WHERE r.txid=o.txid AND r.destination=o.n)
                    OR EXISTS(SELECT 1 FROM soqumap_requests r WHERE r.txid=o.txid AND r.destination=o.n)
                    OR EXISTS(SELECT 1 FROM fatcat_claims r WHERE r.txid=o.txid AND r.destination=o.n)
                    OR EXISTS(SELECT 1 FROM fatcat_requests r WHERE r.txid=o.txid AND r.destination=o.n),
                 a.asset IS NOT NULL AND EXISTS(SELECT 1 FROM listings l WHERE l.txid=o.txid AND l.n=o.n),
                 (SELECT serial FROM fatcat_claims c WHERE c.txid=o.txid AND c.destination=o.n
                   AND NOT EXISTS(SELECT 1 FROM fatcat_pool_claims p WHERE p.txid=c.txid AND p.destination=c.destination))
             FROM outputs o LEFT JOIN asset_notes a USING(txid,n)
             WHERE o.script=?1 AND o.spent_by IS NULL AND o.height>0 ORDER BY o.height,o.txid,o.n LIMIT 10001"
        )?;
        let mut rows = query.query([script])?;
        let mut available = 0u128;
        let mut bonded = 0u128;
        let mut pending = self
            .mempool_outputs_for_wallet(script, true)?
            .iter()
            .try_fold(0u128, |sum, output| {
                Ok::<_, anyhow::Error>(
                    sum + output["value"]
                        .as_str()
                        .context("mempool value")?
                        .parse::<u128>()?,
                )
            })?;
        let mut count = 0;
        let mut coins = Vec::new();
        let mut pending_asset_spends = Vec::new();
        while let Some(row) = rows.next()? {
            count += 1;
            ensure!(
                count <= 10000,
                "address exceeds UTXO limit; pagination required"
            );
            let value: u64 = row.get(2)?;
            let asset: Option<String> = row.get(5)?;
            let quantity: Option<u64> = row.get(6)?;
            if let Some(spender) = row.get::<_, Option<String>>(7)? {
                if let (Some(asset), Some(quantity)) = (asset, quantity) {
                    pending_asset_spends.push(
                        json!({"txid":spender,"asset":asset,"quantity":quantity.to_string()}),
                    );
                }
                continue;
            }
            let height: u64 = row.get(3)?;
            let confirmations = tip.as_ref().map_or(0, |(tip, _)| tip - height + 1);
            let coinbase: bool = row.get(4)?;
            let protected: bool = row.get(8)?;
            let native_spendable = !protected
                && asset.is_none()
                && confirmations >= 1
                && (!coinbase
                    || soq_core::coinbase_maturity(network, height)
                        .is_some_and(|maturity| confirmations >= maturity));

            if confirmations < assets::CONFIRMATIONS {
                pending += u128::from(value);
            } else if protected {
                bonded += u128::from(value);
            } else if native_spendable {
                available += u128::from(value);
            }

            coins.push(json!({
                "txid":row.get::<_,String>(0)?, "vout":row.get::<_,u32>(1)?, "value":value.to_string(), "script":script,
                "height":height,"confirmations":confirmations, "coinbase":coinbase,
                "asset":asset, "quantity":quantity.map(|q| q.to_string()), "protected":protected, "listed":row.get::<_,bool>(9)?,
                "native_spendable":native_spendable,"fatcat_claim":row.get::<_,Option<u16>>(10)?,
            }));
        }
        let activity = self.mempool_history(script)?;
        let mempool_ids = activity.iter().map(|row| &row["txid"]).collect::<Vec<_>>();
        Ok(
            json!({"utxos":coins,"pending_utxos":self.mempool_funding(script)?,
            "confirmed_atoms":(bonded + available).to_string(),"asset_atoms":bonded.to_string(),
            "available_atoms":available.to_string(),"pending_atoms":pending.to_string(),"assets":self.asset_balance(script)?,
            "pending_asset_spends":pending_asset_spends,"activity_mempool_ids":mempool_ids,"discount_nfts":self.discount_nfts(script)?,
            "tip":tip,"includes_mempool":true}),
        )
    }
    pub fn discount_nfts(&self, script: &str) -> Result<Vec<(&'static str, u8)>> {
        let next = self.tip()?.map_or(0, |(height, _)| height + 1);
        assets::discount_nfts(&self.db, script, next, true)
    }
    /// All serials must be backed by a mature pool output or a prior accepted mint.
    pub fn fatcat_availability(&self) -> Result<(u16, u16, bool)> {
        let (minted, halted) = self.fatcat_state()?;
        Ok((fatcat::remaining(&self.db, true)?, minted, halted))
    }

    pub fn fatcat_state(&self) -> Result<(u16, bool)> {
        let minted = self
            .db
            .query_row("SELECT count(*) FROM fatcat_draws", [], |row| row.get(0))?;
        Ok((minted, fatcat::halted(&self.db)?))
    }

    pub fn fatcat_gallery(&self) -> Result<Vec<Value>> {
        let mut q = self.db.prepare(
            "SELECT d.txid,d.variant,d.number,d.metadata_hash,d.height,d.block_hash,o.script,
             EXISTS(SELECT 1 FROM blocks b WHERE b.height=d.height AND b.hash=d.block_hash),t.position,d.metadata
             FROM fatcat_draws d JOIN assets a ON a.id=d.txid JOIN transactions t ON t.txid=d.txid
             LEFT JOIN asset_notes n ON n.asset=d.txid
               AND EXISTS(SELECT 1 FROM outputs x WHERE x.txid=n.txid AND x.n=n.n AND x.spent_by IS NULL)
             LEFT JOIN outputs o ON o.txid=n.txid AND o.n=n.n
             ORDER BY d.height DESC,t.position DESC",
        )?;
        let rows = q.query_map([], |r| Ok(json!({
            "id":r.get::<_,String>(0)?,"kind":"nft","standard":"SOQ-721","virtual_collection":"fatcats","variant":r.get::<_,u16>(1)?,
            "number":r.get::<_,u16>(2)?,"metadata_sha256":r.get::<_,String>(3)?,
            "reveal_height":r.get::<_,u64>(4)?,"reveal_block":r.get::<_,String>(5)?,
            "owner_script":r.get::<_,Option<String>>(6)?,"canonical_reveal":r.get::<_,bool>(7)?,
            "fatcat_metadata":serde_json::from_slice::<Value>(&r.get::<_,Vec<u8>>(9)?).map_err(|error|
            rusqlite::Error::FromSqlConversionFailure(9,rusqlite::types::Type::Blob,Box::new(error)))?
        })))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn fatcat_content(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let data: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT metadata FROM fatcat_draws WHERE metadata_hash=?1",
                [hash],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(bytes) = &data {
            ensure!(
                hex::encode(soq_core::sha256(bytes)) == hash,
                "FATCAT metadata integrity check failed"
            );
        }
        Ok(data)
    }

    pub fn configure_regtest_fatcat_issuer(&self, script: &str) -> Result<()> {
        let decoded = hex::decode(script)?;
        ensure!(
            decoded.len() == 34 && decoded[..2] == [0x51, 32],
            "invalid FATCAT issuer script"
        );
        self.db.execute(
            "INSERT OR IGNORE INTO meta VALUES('fatcat_test_issuer',?1)",
            [script],
        )?;
        let current: String = self.db.query_row(
            "SELECT value FROM meta WHERE key='fatcat_test_issuer'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            current == script,
            "FATCAT issuer differs from the indexed issuer"
        );
        Ok(())
    }

    pub fn asset_balance(&self, script: &str) -> Result<Vec<Value>> {
        assets::balance(&self.db, script)
    }
    pub fn protected(&self, txid: &str, n: u32) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_notes WHERE txid=?1 AND n=?2 UNION SELECT 1 FROM map_requests WHERE txid=?1 AND destination=?2 UNION SELECT 1 FROM soqumap_requests WHERE txid=?1 AND destination=?2 UNION SELECT 1 FROM fatcat_claims WHERE txid=?1 AND destination=?2 UNION SELECT 1 FROM fatcat_requests WHERE txid=?1 AND destination=?2)",
            params![txid, n],
            |r| r.get(0),
        )?)
    }
    pub fn asset_note(&self, txid: &str, n: u32) -> Result<Option<(String, u64)>> {
        Ok(self
            .db
            .query_row(
                "SELECT asset,quantity FROM asset_notes WHERE txid=?1 AND n=?2",
                params![txid, n],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
    pub fn has_output_listing(&self, txid: &str, vout: u32) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM listings WHERE txid=?1 AND n=?2)",
            params![txid, vout],
            |row| row.get(0),
        )?)
    }
    pub fn register_listing(
        &self,
        id: &str,
        offer: &str,
        point: &soq_core::OutPoint,
        asset: &str,
        terms: ListingTerms,
        payout: &soq_core::Output,
    ) -> Result<()> {
        let ListingTerms { quantity, price } = terms;
        self.db.execute(
            "INSERT OR IGNORE INTO listings VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                id,
                point.txid,
                point.vout,
                asset,
                quantity,
                price,
                payout.value,
                hex::encode(&payout.script),
                offer
            ],
        )?;
        let stored: String = self.db.query_row(
            "SELECT id FROM listings WHERE txid=?1 AND n=?2",
            params![point.txid, point.vout],
            |r| r.get(0),
        )?;
        ensure!(
            stored == id,
            "this UTXO already has a signed listing; revoke it on-chain before relisting"
        );
        Ok(())
    }
    pub fn has_listing_receipt(&self, offer: &str, payer: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM listing_receipts WHERE offer=?1 AND payer=?2)",
            params![offer, payer],
            |row| row.get(0),
        )?)
    }
    pub fn listings(&self) -> Result<Vec<Value>> {
        let mut q=self.db.prepare("SELECT l.id,l.txid,l.n,l.asset,l.quantity,l.price,l.offer_json,l.seller_script,EXISTS(SELECT 1 FROM listing_receipts r WHERE r.offer=l.id AND r.payer=l.seller_script) FROM listings l JOIN outputs o ON o.txid=l.txid AND o.n=l.n
            JOIN asset_notes n ON n.txid=l.txid AND n.n=l.n AND n.asset=l.asset AND n.quantity=l.quantity
            WHERE o.spent_by IS NULL AND NOT EXISTS(SELECT 1 FROM mempool_inputs i WHERE i.previous=l.txid AND i.vout=l.n) ORDER BY l.id")?;
        let mut rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"txid":r.get::<_,String>(1)?,"vout":r.get::<_,u32>(2)?,"asset":r.get::<_,String>(3)?,"quantity":r.get::<_,u64>(4)?.to_string(),"price_atoms":r.get::<_,u64>(5)?.to_string(),"offer_json":r.get::<_,String>(6)?,"seller_script":r.get::<_,String>(7)?,"fee_paid":r.get::<_,bool>(8)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut seller_tiers = std::collections::HashMap::new();
        for listing in &mut rows {
            let offer: Value =
                serde_json::from_str(listing["offer_json"].as_str().unwrap_or_default())
                    .unwrap_or(Value::Null);
            let discount = offer["seller_discount_percent"].as_u64().unwrap_or(0);
            if discount > 0 {
                let seller = required_str(listing, "seller_script")?;
                let tier = match seller_tiers.get(seller) {
                    Some(tier) => *tier,
                    None => {
                        let tier = self
                            .discount_nfts(seller)?
                            .first()
                            .map_or(0, |(_, tier)| *tier);
                        seller_tiers.insert(seller.to_owned(), tier);
                        tier
                    }
                };
                listing["discount_valid"] = json!(discount <= u64::from(tier));
            }
            listing["seller_discount_percent"] = json!(discount);
        }
        Ok(rows)
    }
    pub fn trades(&self) -> Result<Vec<Value>> {
        let mut q = self
            .db
            .prepare("SELECT txid,listing,price,quantity FROM trades ORDER BY txid LIMIT 100")?;
        let rows=q.query_map([],|r|Ok(json!({"txid":r.get::<_,String>(0)?,"listing":r.get::<_,String>(1)?,"price_atoms":r.get::<_,u64>(2)?.to_string(),"quantity":r.get::<_,u64>(3)?.to_string()})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn token_markets(&self, now: u64, selected: Option<&str>) -> Result<Value> {
        if let Some(id) = selected {
            hash_bytes(id)?;
        }
        #[derive(Default)]
        struct Market {
            last: Option<(u64, u64)>,
            previous: Option<(u64, u64)>,
            volume_day: u128,
            volume_total: u128,
        }
        let cutoff = now.saturating_sub(86_400);
        let mut markets = std::collections::BTreeMap::<String, Market>::new();
        let mut history = Vec::new();
        let filter = if selected.is_some() {
            " AND l.asset=?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT l.asset,d.txid,d.price,d.quantity,b.time,t.height
             FROM trades d JOIN listings l ON l.id=d.listing JOIN assets a ON a.id=l.asset
             JOIN transactions t ON t.txid=d.txid JOIN blocks b ON b.height=t.height
             WHERE a.kind='token' {filter}
             ORDER BY t.height DESC,t.position DESC",
        );
        let mut query = self.db.prepare_cached(&sql)?;
        let mut rows = query.query(rusqlite::params_from_iter(selected))?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let price: u64 = row.get(2)?;
            let quantity: u64 = row.get(3)?;
            let time: u64 = row.get(4)?;
            let market = markets.entry(id).or_default();
            market.last.get_or_insert((price, quantity));
            market.volume_total += u128::from(price);
            if time > cutoff {
                market.volume_day += u128::from(price);
            } else {
                market.previous.get_or_insert((price, quantity));
            }
            if selected.is_some() && history.len() < 200 {
                history.push(json!({
                    "txid":row.get::<_,String>(1)?, "price_atoms":price.to_string(),
                    "quantity":quantity.to_string(), "time":time, "height":row.get::<_,u64>(5)?
                }));
            }
        }
        let filter = if selected.is_some() {
            " AND a.id=?1"
        } else {
            ""
        };
        let sql = format!(
            "SELECT a.id,coalesce(sum(CASE WHEN o.txid IS NOT NULL THEN n.quantity END),0),
                    count(DISTINCT o.script)
             FROM assets a LEFT JOIN asset_notes n ON n.asset=a.id
             LEFT JOIN outputs o ON o.txid=n.txid AND o.n=n.n AND o.spent_by IS NULL
             WHERE a.kind='token' {filter} GROUP BY a.id",
        );
        let mut supply = self.db.prepare_cached(&sql)?;
        let mut rows = supply.query(rusqlite::params_from_iter(selected))?;
        let mut result = serde_json::Map::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let mut value = json!({"circulating_units":row.get::<_,u64>(1)?.to_string(),"holders":row.get::<_,u64>(2)?});
            if let Some(market) = markets.remove(&id) {
                let quote = |(price, quantity): (u64, u64)| json!({"price_atoms":price.to_string(),"quantity":quantity.to_string()});
                value["last"] = json!(market.last.map(quote));
                value["previous"] = json!(market.previous.map(quote));
                value["volume_day_atoms"] = json!(market.volume_day.to_string());
                value["volume_total_atoms"] = json!(market.volume_total.to_string());
            }
            result.insert(id, value);
        }
        let mut snapshot = json!({"markets":result,"history":history});
        if selected.is_none() {
            let mut collection_volume = std::collections::BTreeMap::<String, u128>::new();
            let mut query = self.db.prepare(
                "SELECT n.collection,d.price FROM trades d JOIN listings l ON l.id=d.listing
                 JOIN collection_nfts n ON n.txid=l.asset
                 UNION ALL SELECT 'fatcats',d.price FROM trades d JOIN listings l ON l.id=d.listing
                 JOIN fatcat_draws f ON f.txid=l.asset",
            )?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let collection: String = row.get(0)?;
                let price: u64 = row.get(1)?;
                *collection_volume.entry(collection).or_default() += u128::from(price);
            }
            snapshot["collection_volume_atoms"] = json!(collection_volume
                .into_iter()
                .map(|(id, volume)| (id, volume.to_string()))
                .collect::<std::collections::BTreeMap<_, _>>());
        }
        Ok(snapshot)
    }
    pub fn record_submission(
        &mut self,
        transaction: &soq_core::Transaction,
        raw: &str,
    ) -> Result<()> {
        let id = transaction.txid()?;
        if let Ok(assets::Operation::Listing { offer }) =
            soq_core::protocol::operation(&transaction.outputs)
        {
            let input = &transaction
                .inputs
                .first()
                .context("listing fee input missing")?
                .previous;
            let payer: String = self.db.query_row(
                "SELECT script FROM outputs WHERE txid=?1 AND n=?2",
                params![input.txid, input.vout],
                |row| row.get(0),
            )?;
            let offer = hex::encode(offer);
            ensure!(
                !self.has_listing_receipt(&offer, &payer)?,
                "listing fee is already paid"
            );
            let t = self.db.transaction()?;
            t.execute("DELETE FROM listing_submissions WHERE offer=?1 AND payer=?2 AND txid IN (SELECT txid FROM pending WHERE state='not_in_mempool')", params![offer, payer])?;
            t.execute(
                "INSERT INTO pending VALUES(?1,?2,'prepared') ON CONFLICT(txid) DO UPDATE SET raw=excluded.raw,state='prepared' WHERE pending.state='not_in_mempool'",
                params![id, raw],
            )?;
            ensure!(
                t.execute(
                    "INSERT OR IGNORE INTO listing_submissions VALUES(?1,?2,?3)",
                    params![offer, payer, id]
                )? == 1,
                "a listing fee for this offer is already pending"
            );
            t.commit()?;
            Ok(())
        } else {
            self.record_pending(&id, raw)
        }
    }
    pub fn record_pending(&self, id: &str, raw: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO pending(txid,raw,state) VALUES(?1,?2,'prepared') ON CONFLICT(txid) DO UPDATE SET raw=excluded.raw,state='prepared' WHERE pending.state='not_in_mempool'",
            params![id, raw],
        )?;
        Ok(())
    }
    pub fn pending_state(&self, id: &str, state: &str) -> Result<()> {
        self.db.execute(
            "UPDATE pending SET state=?2 WHERE txid=?1",
            params![id, state],
        )?;
        Ok(())
    }
    pub fn pending(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT state FROM pending WHERE txid=?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn known_mempool(&self) -> Result<std::collections::HashSet<String>> {
        let mut q = self.db.prepare("SELECT txid FROM observed_mempool")?;
        let ids = q
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    pub fn mempool_funding(&self, script: &str) -> Result<Vec<Value>> {
        self.mempool_outputs_for_wallet(script, false)
    }

    fn mempool_outputs_for_wallet(
        &self,
        script: &str,
        include_protected: bool,
    ) -> Result<Vec<Value>> {
        let mut q = self.db.prepare(
            "SELECT o.txid,o.n,o.value,o.protected FROM mempool_outputs o WHERE o.script=?1 AND o.value>0 AND (?2 OR o.protected=0)
             AND NOT EXISTS(SELECT 1 FROM transactions t WHERE t.txid=o.txid)
             AND NOT EXISTS(SELECT 1 FROM mempool_inputs i WHERE i.previous=o.txid AND i.vout=o.n)"
        )?;
        let outputs = q.query_map(params![script, include_protected], |r| Ok(json!({
            "txid":r.get::<_, String>(0)?, "vout":r.get::<_,u32>(1)?, "value":r.get::<_,u64>(2)?.to_string(),
            "script":script,"confirmations":0,"coinbase":false,"protected":r.get::<_,bool>(3)?
        })))?.collect::<rusqlite::Result<_>>()?;
        Ok(outputs)
    }

    pub fn mempool_spender(&self, txid: &str, vout: u32) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT txid FROM mempool_inputs WHERE previous=?1 AND vout=?2 LIMIT 1",
                params![txid, vout],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn mempool_spends(&self, txid: &str, vout: u32) -> Result<bool> {
        Ok(self.mempool_spender(txid, vout)?.is_some())
    }

    pub fn observe_mempool(&mut self, ids: &[String]) -> Result<()> {
        self.observe_mempool_transactions(ids, &[])
    }

    pub fn observe_mempool_transactions(
        &mut self,
        ids: &[String],
        decoded: &[Value],
    ) -> Result<()> {
        let t = self.db.transaction()?;
        t.execute(
            "DELETE FROM mempool WHERE txid NOT IN (SELECT value FROM json_each(?1))",
            [serde_json::to_string(ids)?],
        )?;
        t.execute(
            "UPDATE pending SET state='not_in_mempool' WHERE state IN ('broadcast','mempool','reorganized') AND txid NOT IN (SELECT value FROM json_each(?1))",
            [serde_json::to_string(ids)?],
        )?;
        for id in ids {
            soq_core::hash_bytes(id)?;
            t.execute("INSERT OR IGNORE INTO mempool VALUES(?1)", [id])?;
            t.execute(
                "UPDATE pending SET state='mempool' WHERE txid=?1 AND state!='confirmed'",
                [id],
            )?;
        }
        for body in decoded {
            let id = required_str(body, "txid")?;
            let inputs = body["vin"]
                .as_array()
                .context("mempool vin")?
                .iter()
                .map(|input| {
                    Ok((
                        required_str(input, "txid")?,
                        input["vout"].as_u64().context("mempool vout")?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            let outputs = body["vout"].as_array().context("mempool outputs")?;
            let outputs = outputs
                .iter()
                .map(|output| {
                    Ok(soq_core::Output {
                        script: hex::decode(required_str(&output["scriptPubKey"], "hex")?)?,
                        value: atoms(&output["value"].to_string())?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Self::insert_mempool_body(&t, id, inputs, &outputs)?;
        }
        t.commit()?;
        Ok(())
    }

    pub fn accept_submission(&mut self, tx: &soq_core::Transaction) -> Result<()> {
        let id = tx.txid()?;
        let t = self.db.transaction()?;
        if !t.query_row(
            "SELECT EXISTS(SELECT 1 FROM transactions WHERE txid=?1)",
            [&id],
            |r| r.get::<_, bool>(0),
        )? {
            t.execute("INSERT OR IGNORE INTO mempool VALUES(?1)", [&id])?;
            Self::insert_mempool_body(
                &t,
                &id,
                tx.inputs
                    .iter()
                    .map(|input| (input.previous.txid.as_str(), u64::from(input.previous.vout))),
                &tx.outputs,
            )?;
            t.execute("UPDATE pending SET state='broadcast' WHERE txid=?1", [&id])?;
        }
        t.commit()?;
        Ok(())
    }

    fn insert_mempool_body<'a>(
        t: &rusqlite::Transaction<'_>,
        id: &str,
        inputs: impl IntoIterator<Item = (&'a str, u64)>,
        outputs: &[soq_core::Output],
    ) -> Result<()> {
        if t.execute("INSERT OR IGNORE INTO observed_mempool VALUES(?1)", [id])? == 0 {
            return Ok(());
        }
        for (n, (txid, vout)) in inputs.into_iter().enumerate() {
            t.execute(
                "INSERT INTO mempool_inputs VALUES(?1,?2,?3,?4)",
                params![id, n, txid, vout],
            )?;
        }
        let destinations = match soq_core::protocol::operation(outputs) {
            Ok(
                assets::Operation::Nft { destination, .. }
                | assets::Operation::CollectionNft { destination, .. }
                | assets::Operation::Mint { destination, .. }
                | assets::Operation::MapRequest { destination }
                | assets::Operation::SoquMapRequest { destination }
                | assets::Operation::FatcatClaim { destination, .. }
                | assets::Operation::FatcatRequest { destination }
                | assets::Operation::FatcatMint { destination }
                | assets::Operation::FatcatPoolMint { destination },
            ) => vec![destination],
            Ok(assets::Operation::FatcatPoolClaim { count, .. }) => (0..count).collect(),
            Ok(assets::Operation::Settle { .. }) => vec![1],
            Ok(assets::Operation::Cancel { .. }) => vec![0],
            Ok(assets::Operation::Transfer { allocations, .. }) => {
                allocations.into_iter().map(|(n, _)| n).collect()
            }
            _ => vec![],
        };
        for (n, output) in outputs.iter().enumerate() {
            let protected = u8::try_from(n).is_ok_and(|n| destinations.contains(&n));
            t.execute(
                "INSERT INTO mempool_outputs VALUES(?1,?2,?3,?4,?5)",
                params![id, n, output.value, hex::encode(&output.script), protected],
            )?;
        }
        Ok(())
    }
    pub fn transaction_status(&self, id: &str) -> Result<Value> {
        let height: Option<u64> = self
            .db
            .query_row("SELECT height FROM transactions WHERE txid=?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let tip = self.tip()?.map(|t| t.0);
        let reason: Option<String> = self
            .db
            .query_row(
                "SELECT reason FROM invalid_asset_operations WHERE txid=?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let mut status = json!({"txid":id,"state":self.pending(id)?,"confirmations":height.zip(tip).map(|(h,tip)|tip-h+1).unwrap_or(0),"asset_operation_error":reason});
        let map: Option<(Option<u64>, bool)> = self.db.query_row(
            "SELECT a.height,m.txid IS NOT NULL FROM soqumap_requests r
             LEFT JOIN soqumap_assignments a USING(txid) LEFT JOIN soqumaps m USING(txid) WHERE r.txid=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if let Some((height, revealed)) = map {
            status["map"] = json!({"assigned_height":height,"revealed":revealed});
        }
        if let Some(fatcat) = fatcat::status(&self.db, id)? {
            status["fatcat"] = fatcat;
        }
        Ok(status)
    }

    pub fn submitted_action_scope(&self, id: &str) -> Result<Value> {
        let raw: Option<String> = self
            .db
            .query_row("SELECT raw FROM pending WHERE txid=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(tx) = raw
            .and_then(|raw| hex::decode(raw).ok())
            .and_then(|raw| soq_core::Transaction::decode(&raw).ok())
        else {
            return Ok(Value::Null);
        };
        use assets::Operation;
        let scope = match soq_core::protocol::operation(&tx.outputs) {
            Ok(Operation::Mint { token, .. }) => json!({"asset":hex::encode(token)}),
            Ok(Operation::CollectionNft { collection, .. }) => {
                json!({"asset":hex::encode(collection)})
            }
            Ok(Operation::Transfer { asset, .. }) => json!({"asset":hex::encode(asset)}),
            Ok(Operation::Settle { asset, .. }) => {
                let point = &tx
                    .inputs
                    .first()
                    .context("missing settlement input")?
                    .previous;
                let offer: Option<String> = self
                    .db
                    .query_row(
                        "SELECT id FROM listings WHERE txid=?1 AND n=?2",
                        params![point.txid, point.vout],
                        |row| row.get(0),
                    )
                    .optional()?;
                json!({"asset":hex::encode(asset),"offer":offer})
            }
            Ok(Operation::Cancel { .. }) => {
                let point = &tx
                    .inputs
                    .first()
                    .context("missing cancellation input")?
                    .previous;
                json!({"point":format!("{}:{}", point.txid, point.vout)})
            }
            Ok(Operation::Listing { offer }) => self
                .db
                .query_row(
                    "SELECT txid,n,asset FROM listings WHERE id=?1",
                    [hex::encode(offer)],
                    |row| {
                        let txid: String = row.get(0)?;
                        let vout: u32 = row.get(1)?;
                        let asset: String = row.get(2)?;
                        Ok(json!({"point":format!("{txid}:{vout}"),"asset":asset}))
                    },
                )
                .optional()?
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };
        Ok(scope)
    }

    pub fn check_creation(&self, kind: &str, key: &str, ticker: Option<&str>) -> Result<()> {
        assets::check_confirmed_creation(&self.db, kind, Some(key), ticker)?;
        let mut candidates = self.db.prepare(
            "SELECT txid,min(script) FROM mempool_outputs WHERE substr(script,1,2)='6a' GROUP BY txid HAVING count(*)=1"
        )?;
        let mut paid = self.db.prepare(
            "SELECT count(*),min(value) FROM mempool_outputs WHERE txid=?1 AND script=?2",
        )?;
        let mut rows = candidates.query([])?;
        while let Some(row) = rows.next()? {
            let txid: String = row.get(0)?;
            let script: String = row.get(1)?;
            let Ok(op) = assets::Operation::parse(&hex::decode(script)?) else {
                continue;
            };
            let Some(
                fee_kind @ (soq_core::fees::MintKind::TokenDeploy
                | soq_core::fees::MintKind::Collection),
            ) = op.platform_kind()
            else {
                continue;
            };
            let fee = soq_core::fees::fee_output(fee_kind)?;
            let (count, value): (u64, Option<u64>) = paid
                .query_row(params![txid, hex::encode(fee.script)], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?;
            let tier = if value == Some(fee.value) {
                0
            } else {
                let owner: Option<String> = self.db.query_row(
                    "SELECT o.script FROM mempool_inputs m JOIN outputs o ON o.txid=m.previous AND o.n=m.vout
                     WHERE m.txid=?1 AND m.n=0", [&txid], |row| row.get(0)
                ).optional()?;
                match owner {
                    Some(owner) => self
                        .discount_nfts(&owner)?
                        .first()
                        .map_or(0, |(_, percent)| *percent),
                    None => 0,
                }
            };
            if count == 1
                && value.is_some_and(|paid| soq_core::fees::authorized_fee(paid, fee.value, tier))
            {
                Self::check_pending_creation(&op, kind, key, ticker)?;
            }
        }
        let mut candidates = self
            .db
            .prepare("SELECT raw FROM pending WHERE state IN ('prepared','broadcast')")?;
        let mut rows = candidates.query([])?;
        while let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            if let Ok(tx) = hex::decode(raw)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| soq_core::Transaction::decode(&bytes))
            {
                if let Ok(op) = soq_core::protocol::operation(&tx.outputs) {
                    Self::check_pending_creation(&op, kind, key, ticker)?;
                }
            }
        }
        Ok(())
    }

    fn check_pending_creation(
        op: &assets::Operation,
        kind: &str,
        key: &str,
        ticker: Option<&str>,
    ) -> Result<()> {
        if let Ok(Some((pending_kind, pending_key, pending_ticker))) = assets::creation_claim(op) {
            ensure!(
                pending_kind != kind || pending_key.as_deref() != Some(key),
                "asset name already has a pending creation"
            );
            ensure!(
                ticker.is_none() || pending_ticker.as_deref() != ticker,
                "token ticker already has a pending deployment"
            );
        }
        Ok(())
    }

    /// Execute the same asset validator in a rollback-only SQL transaction before broadcasting.
    pub fn preflight(&mut self, transaction: &soq_core::Transaction) -> Result<()> {
        let id = transaction.txid()?;
        let (height, _) = self.tip()?.context("index is empty")?;
        let op = if assets::has_protocol(&transaction.outputs) {
            Some(soq_core::protocol::operation(&transaction.outputs)?)
        } else {
            None
        };
        if let Some(op) = &op {
            if let Some((kind, key, ticker)) = assets::creation_claim(op)? {
                let key = key.context("creation requires an on-chain name commitment")?;
                self.check_creation(kind, &key, ticker.as_deref())?;
            }
        }
        if let Some(assets::Operation::Settle {
            asset,
            quantity,
            price,
        }) = &op
        {
            let point = &transaction
                .inputs
                .first()
                .context("asset input missing")?
                .previous;
            let payout = transaction
                .outputs
                .first()
                .context("seller payout missing")?;
            let paid: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM listings l JOIN listing_receipts r ON r.offer=l.id AND r.payer=l.seller_script
                 WHERE l.txid=?1 AND l.n=?2 AND l.asset=?3 AND l.quantity=?4 AND l.price=?5
                   AND l.payout=?6 AND l.seller_script=?7)",
                params![point.txid, point.vout, hex::encode(asset),quantity,price,payout.value,hex::encode(&payout.script)],
                |row| row.get(0),
            )?;
            ensure!(paid, "settlement requires an active paid listing");
        }
        for (position, input) in transaction.inputs.iter().enumerate() {
            if matches!(&op, Some(assets::Operation::Transfer { .. })) {
                ensure!(
                    !self.has_output_listing(&input.previous.txid, input.previous.vout)?,
                    "cancel the signed listing before transferring that lot"
                );
            }
            if self.protected(&input.previous.txid, input.previous.vout)? {
                let allowed=match &op{Some(assets::Operation::FatcatMint{..}|assets::Operation::FatcatPoolMint{..}) if position==0=>self.db.query_row("SELECT EXISTS(SELECT 1 FROM fatcat_claims WHERE txid=?1 AND destination=?2)",params![input.previous.txid,input.previous.vout],|row| row.get(0))?, Some(assets::Operation::Transfer{asset,..} | assets::Operation::Settle{asset,..} | assets::Operation::Cancel{asset,..})=>self.db.query_row("SELECT EXISTS(SELECT 1 FROM asset_notes WHERE txid=?1 AND n=?2 AND asset=?3)",params![input.previous.txid,input.previous.vout,hex::encode(asset)],|r|r.get::<_,bool>(0))?,_=>false};
                ensure!(
                    allowed,
                    "transaction would destroy an asset or pending artwork request"
                );
            }
        }
        let t = self.db.transaction()?;
        t.execute(
            "INSERT INTO transactions VALUES(?1,?2,2147483647)",
            params![id, height],
        )?;
        for input in &transaction.inputs {
            ensure!(
                t.execute(
                    "UPDATE outputs SET spent_by=?1 WHERE txid=?2 AND n=?3 AND spent_by IS NULL",
                    params![id, input.previous.txid, input.previous.vout]
                )? == 1,
                "input unavailable in canonical index"
            );
        }
        for (n, o) in transaction.outputs.iter().enumerate() {
            t.execute(
                "INSERT INTO outputs VALUES(?1,?2,?3,?4,?5,0,NULL)",
                params![id, n, o.value, hex::encode(&o.script), height],
            )?;
        }
        if op.is_some() {
            let prevouts: Vec<_> = transaction
                .inputs
                .iter()
                .map(|input| input.previous.clone())
                .collect();
            assets::apply(
                &t,
                &id,
                &prevouts,
                &transaction.outputs,
                false,
                height + 1,
                true,
            )?;
        }
        t.rollback()?;
        Ok(())
    }
    pub fn block_hash(&self, height: u64) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT hash FROM blocks WHERE height=?1", [height], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn recent(&self) -> Result<Vec<Value>> {
        let mut q = self.db.prepare(
            "SELECT txid,height FROM transactions ORDER BY height DESC,position DESC LIMIT 50",
        )?;
        let rows = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.iter()
            .map(|(id, height)| self.summary(id, *height))
            .collect()
    }
    pub fn verify(&self) -> Result<()> {
        let result: String = self
            .db
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        ensure!(result == "ok", "SQLite integrity failure");
        let bad: u64 =
            self.db
                .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })?;
        ensure!(bad == 0, "foreign key inconsistency");
        Ok(())
    }
}
// The window bounds both concurrency and the number of decoded blocks retained.
const BLOCK_WINDOW: u64 = 4;

pub async fn next_window(rpc: &Rpc, tip: Option<(u64, String)>) -> Result<Sync> {
    let info = rpc.call("getblockchaininfo", json!([])).await?;
    let node_height = info["blocks"].as_u64().context("node height")?;
    let best = required_str(&info, "bestblockhash")?;
    let node_ready = info["initialblockdownload"].as_bool() == Some(false);
    if let Some((height, hash)) = &tip {
        if *height > node_height {
            return Ok(Sync::Rollback(*height));
        }
        if *height == node_height && hash == best {
            return Ok(Sync::Current {
                node_height,
                node_ready,
            });
        }
        if rpc.hash(*height).await? != *hash {
            ensure!(*height > 0, "node genesis differs from the indexed chain");
            return Ok(Sync::Rollback(*height));
        }
    }
    let first = tip.as_ref().map_or(0, |(height, _)| height + 1);
    if first > node_height {
        return Ok(Sync::Retry);
    }
    let last = node_height.min(first.saturating_add(BLOCK_WINDOW - 1));
    let hashes = rpc.hashes(first, last).await?;
    let mut requests = tokio::task::JoinSet::new();
    for (offset, hash) in hashes.iter().cloned().enumerate() {
        let rpc = rpc.clone();
        requests.spawn(async move {
            let mut block = rpc.call("getblock", json!([hash, 2])).await?;
            ensure!(
                block["height"].as_u64() == Some(first + offset as u64) && block["hash"] == hash,
                "RPC block identity mismatch"
            );
            // The node's detailed JSON includes witness bytes and formatted
            // scripts that block application does not consume.
            if let Some(transactions) = block["tx"].as_array_mut() {
                for tx in transactions {
                    if let Some(fields) = tx.as_object_mut() {
                        fields.retain(|key, _| matches!(key.as_str(), "txid" | "vin" | "vout"));
                    }
                    if let Some(inputs) = tx.get_mut("vin").and_then(Value::as_array_mut) {
                        for input in inputs {
                            if let Some(fields) = input.as_object_mut() {
                                fields.retain(|key, _| {
                                    matches!(key.as_str(), "txid" | "vout" | "coinbase")
                                });
                            }
                        }
                    }
                }
            }
            Ok::<_, anyhow::Error>((offset, block))
        });
    }
    let mut blocks = vec![Value::Null; hashes.len()];
    while let Some(result) = requests.join_next().await {
        let (offset, block) = result??;
        blocks[offset] = block;
    }
    let mut parent = tip.as_ref().map(|(_, hash)| hash.as_str()).unwrap_or("");
    for (block, hash) in blocks.iter().zip(&hashes) {
        if block["previousblockhash"].as_str().unwrap_or("") != parent {
            return Ok(Sync::Retry);
        }
        parent = hash;
    }
    // A canonical last hash anchors the entire contiguous, parent-checked window.
    if rpc.hash(last).await? != parent {
        return Ok(Sync::Retry);
    }
    Ok(Sync::Blocks {
        blocks,
        node_height,
    })
}

pub enum Sync {
    Blocks {
        blocks: Vec<Value>,
        node_height: u64,
    },
    Rollback(u64),
    Current {
        node_height: u64,
        node_ready: bool,
    },
    Retry,
}
#[cfg(test)]
mod tests {
    use super::*;
    fn block(h: u64, hash: &str, parent: &str, txs: Value) -> Value {
        json!({"height":h,"hash":hash,"previousblockhash":parent,"time":1,"tx":txs})
    }
    fn coinbase(id: &str) -> Value {
        json!({"txid":id,"vin":[{"coinbase":"00"}],"vout":[{"n":0,"value":50,"scriptPubKey":{"hex":"5120"}}]})
    }
    #[test]
    fn sqlite_readers_hold_a_stable_snapshot_across_writer_commits() {
        assert!(rusqlite::version_number() >= 3_051_003);
        let path =
            std::env::temp_dir().join(format!("soq-read-snapshot-{}.sqlite", std::process::id()));
        let writer = Store::open(&path).unwrap();
        let reader = Store::open_read_only(&path).unwrap();
        reader
            .snapshot(|store| {
                assert_eq!(store.tip()?, None);
                writer
                    .db
                    .execute("INSERT INTO blocks VALUES(0,'test','',0)", [])?;
                assert_eq!(store.tip()?, None);
                assert!(store.db.execute("DELETE FROM blocks", []).is_err());
                Ok(())
            })
            .unwrap();
        assert_eq!(reader.tip().unwrap(), Some((0, "test".into())));
        drop(reader);
        drop(writer);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn integer_upgrade_removes_legacy_token_and_preserves_other_assets() {
        let mut store = Store::open(":memory:").unwrap();
        let genesis = "ab".repeat(32);
        let token = "cd".repeat(32);
        let nft = "ef".repeat(32);
        store
            .apply(&block(0, &genesis, "", json!([coinbase(&genesis)])))
            .unwrap();
        store
            .db
            .execute("INSERT INTO transactions VALUES(?1,0,1)", [&token])
            .unwrap();
        store
            .db
            .execute("INSERT INTO transactions VALUES(?1,0,2)", [&nft])
            .unwrap();
        store
            .db
            .execute(
                "ALTER TABLE assets ADD COLUMN decimals INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO assets VALUES(?1,'token',?1,10,10,'TEST',0)",
                [&token],
            )
            .unwrap();
        store
            .db
            .execute("INSERT INTO assets VALUES(?1,'nft',?1,1,1,'',0)", [&nft])
            .unwrap();
        store
            .db
            .execute("INSERT INTO asset_events VALUES(?1,'{}')", [&token])
            .unwrap();
        store
            .db
            .execute("INSERT INTO asset_mints VALUES(?1,?1,10)", [&token])
            .unwrap();
        Store::upgrade_integer_tokens(&store.db).unwrap();
        Store::upgrade_integer_tokens(&store.db).unwrap();
        assert!(store.asset(&token).is_err());
        assert_eq!(store.asset(&nft).unwrap()["kind"], "nft");
        assert_eq!(store.transaction_location(&token).unwrap().0, 0);
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM asset_events", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM asset_mints", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert!(store.db.prepare("SELECT decimals FROM assets").is_err());
        store.verify().unwrap();
    }

    #[test]
    fn failed_window_leaves_no_partially_committed_blocks() {
        let mut store = Store::open(":memory:").unwrap();
        let genesis = "01".repeat(32);
        let next = "02".repeat(32);
        let genesis_block = block(0, &genesis, "", json!([coinbase(&genesis)]));
        let bad_block = block(1, &next, &genesis, json!([]));
        assert!(store
            .apply_window(&[genesis_block.clone(), bad_block])
            .is_err());
        assert!(store.tip().unwrap().is_none());
        store
            .apply_window(&[
                genesis_block,
                block(1, &next, &genesis, json!([coinbase(&next)])),
            ])
            .unwrap();
        assert_eq!(store.tip().unwrap(), Some((1, next)));
        store.verify().unwrap();
    }

    #[test]
    fn reorg_atomic_and_idempotent() {
        let mut s = Store::open(":memory:").unwrap();
        let a = "01".repeat(32);
        let b = "02".repeat(32);
        let c = "03".repeat(32);
        let t = "aa".repeat(32);
        let u = "bb".repeat(32);
        s.apply(&block(0, &a, "", json!([coinbase(&t)]))).unwrap();
        s.apply(&block(0, &a, "", json!([coinbase(&t)]))).unwrap();
        let spend = json!({"txid":u,"vin":[{"txid":t,"vout":0}],"vout":[{"n":0,"value":49,"scriptPubKey":{"hex":"5121"}}]});
        s.apply(&block(1, &b, &a, json!([coinbase(&c), spend])))
            .unwrap();
        assert!(s.coins("5120").unwrap().iter().all(|c| c.txid != t));
        s.rollback(1).unwrap();
        assert_eq!(
            s.db.query_row("SELECT spent_by FROM outputs WHERE txid=?1", [&t], |r| {
                r.get::<_, Option<String>>(0)
            })
            .unwrap(),
            None
        );
        assert!(s.coins("5120").unwrap().is_empty());
        assert!(s
            .apply(&block(
                1,
                &b,
                &a,
                json!([coinbase(&c),{"txid":u,"vin":[{"txid":"ff".repeat(32),"vout":0}],"vout":[]}])
            ))
            .is_err());
        assert_eq!(s.tip().unwrap().unwrap().0, 0);
        s.verify().unwrap();
    }
}
