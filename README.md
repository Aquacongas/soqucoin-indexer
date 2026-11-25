# Soqucoin Indexer and SOQ.MARKET Wallet

## Project overview

Soqucoin Indexer builds a local SQLite index of the active chain reported by
Soqucoin Core. It records block identities and timestamps, transaction IDs and
block positions, output scripts and amounts, and spending relationships. The
ledger also interprets SQR-20, SOQ-721, SOQ-FINGERPRINT, SOQ-MAPS, and FATCAT
records. Follow mode observes the node's mempool.

The indexer can catch up while the node is downloading blocks. A complete
current index requires the node to finish initial block download, and the
indexer's one-shot mode waits for that condition. Soqucoin Core owns consensus
validation. The release contains a command-line process and a Rust library.

The release also includes the SOQ.MARKET wallet engine. It derives ML-DSA-44
keys, constructs and signs transactions in Rust, and exposes the same wallet
through WebAssembly. The browser integration uses the marketplace APIs for
funding data, network fee quotes and broadcasting.

[On-chain protocols and wallet architecture](PROTOCOL%26WALLET.md)

### Source layout

| Path | Contents |
| --- | --- |
| `crates/soq-indexer` | Synchronization, SQLite ledger and CLI |
| `crates/soq-core` | Shared transaction serialization, scripts, SQMK operations and fees |
| `crates/soq-wallet` | Wallet, recovery, signing, marketplace and WASM bindings |
| `crates/soq-fingerprints`, `crates/soq-maps` | Deterministic metadata and artwork |
| `wallet-browser` | Browser storage, fee quotation and submission integration |
| `tests`, `vendor` | Verification fixtures, WASM tests and pinned dependencies |

### Wallet build and tests

After installing Rust 1.96.0, run the following from the release root. The
conformance tests use Node.js 22.22.1 and synthetic UTXOs.

```sh
rustup target add --toolchain 1.96.0 wasm32-unknown-unknown
cargo +1.96.0 install wasm-bindgen-cli --locked --version 0.2.122
cargo +1.96.0 test --locked --workspace
cargo +1.96.0 build --locked -p soq-wallet --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/soq_wallet.wasm --target web --out-dir target/wallet-web
node tests/browser-wallet.mjs
node tests/fatcat-wallet.mjs
```

An unlocked wallet trusts the browser and the JavaScript delivered to its
origin. The backup APIs can pass recovery secrets into JavaScript. The
production browser stores an encrypted wallet envelope in local storage.
The marketplace supplies wallet-visible chain state, quotes, and broadcast
responses. See the protocol and wallet document for the data and trust
boundaries.

## Architecture

`Soqucoin Core → JSON-RPC → soq-indexer → SQLite → SQL or Rust queries`

- `Rpc` connects to a numeric loopback address over HTTP with Basic
  authentication. It pools connections and bounds response sizes and time.
- `next_window` compares the local checkpoint with the node's chain and
  retrieves blocks with `getblock` verbosity 2. `Store` applies windows and
  rollbacks using SQLite transactions.
- `soq-core` supplies amount, script, transaction, and asset-protocol parsing.
  `soq-fingerprints` and `soq-maps` generate deterministic asset metadata
  and content during ledger processing.
- The command-line process runs synchronization. Read-oriented Rust
  consumers can use `Store::open_read_only` and `Store::snapshot`, or
  inspect the database with SQLite.

## Requirements

- Linux x86-64 is the tested platform.
- Rust and Cargo 1.96.0 are the tested toolchain. The code uses
  `std::fs::File::try_lock`, which requires Rust 1.89 or newer. Earlier
  toolchains than the tested version have not been validated.
- A C compiler and linker for bundled SQLite 3.54.1. Cargo builds SQLite
  from the included amalgamation. A system SQLite development package is
  unnecessary. The standalone `sqlite3` client is useful for the checks below.
- Registry access on the first build, or a populated Cargo cache.
- A running, unpruned [Soqucoin Core node](https://github.com/soqucoin/soqucoin)
  with `server=1`, `txindex=1`, and working loopback RPC credentials.
  The startup probe relies on this Core RPC family's `getrawtransaction`
  missing-transaction error to distinguish a node without `txindex`.

## Building from source

On Debian or Ubuntu, install the native build tools and optional SQLite client.
Install Rust with [rustup](https://rustup.rs/) if needed.

```sh
sudo apt-get update
sudo apt-get install build-essential sqlite3
rustup toolchain install 1.96.0
cargo +1.96.0 build --release --locked
./target/release/soq-indexer --help
```

Run the Cargo and binary commands from the checkout root. Once the locked
dependencies are cached, the build also accepts `--offline`.

## Configuration

Set these environment variables before launching the indexer. The application
reads its process environment. A shell can load the example with
`. ./.env.example`.

| Setting | Default | Accepted values and purpose |
|---|---|---|
| `SOQ_RPC_URL` | `http://127.0.0.1:9999` | HTTP URL with a numeric loopback IP and explicit port, such as `http://127.0.0.1:9999` or `http://[::1]:9999`. RPC requests go to `/`. Match the port configured on the node. |
| `SOQ_RPC_USER` | Required | Soqucoin Core RPC username for HTTP Basic authentication. |
| `SOQ_RPC_PASSWORD` | Required | Corresponding RPC password. Supply it through a protected service environment or secret store. |
| `SOQ_DATABASE` | `soq-index.sqlite` in the working directory | Relative or absolute path to a SQLite file. Its parent directory must exist and be writable. Use a separate file for each chain. |
| `SOQ_NETWORK` | `main` | Case-sensitive `main`, `regtest`, or `stagenet`. Selects the expected genesis. The RPC port is configured separately. |
| `SOQ_GENESIS` | Selected network's genesis | Optional expected genesis override, as the exact 64-character hexadecimal hash returned by `getblockhash 0`. Use only for a separately verified chain. |

The indexer's port default follows its existing configuration. Set
`SOQ_RPC_URL` explicitly if Core uses a different port. For example, Core
v2.6.1.0 advertises 33389 as its mainnet RPC default. The indexer sends
the configured username and password directly and does not read a cookie file.

## Command-line reference

| Flag | Description | Default | Required |
|---|---|---|---|
| `--follow` | Continue after catching up and refresh the mempool when the indexed tip matches the node. Boolean switch with no value. | Absent. Run once until caught up to a ready node. | No |
| `--help` | Print usage and environment settings, then exit before connecting or opening a database. Boolean switch with no value. | Absent. Run synchronization. | No |

Pass at most one flag. The process writes progress to stdout and errors or
rollback reports to stderr.

## Quick start

1. Install the tools and build the indexer as described above.
2. Configure a local Soqucoin Core node. For a node you are setting up on
   the indexer's default port, place settings like these in Core's
   `soqucoin.conf`. Replace the placeholders with your credentials.

   ```ini
   server=1
   txindex=1
   prune=0
   rpcallowip=127.0.0.1
   rpcbind=127.0.0.1
   rpcport=9999
   rpcuser=indexer-rpc-user
   rpcpassword=replace-with-a-strong-unique-password
   ```

   Start Core and check `soqucoin-cli getblockchaininfo` using its
   configuration. If enabling `txindex` on an existing node, follow Core's
   instructions for rebuilding its transaction index.
3. Configure and run the indexer from the checkout. With Bash, the prompts
   keep the password out of command history.

   ```bash
   umask 077
   mkdir -p data
   . ./.env.example
   read -r -p 'RPC user: ' SOQ_RPC_USER
   read -r -s -p 'RPC password: ' SOQ_RPC_PASSWORD
   printf '\n'
   export SOQ_RPC_USER SOQ_RPC_PASSWORD
   ./target/release/soq-indexer
   ```

4. Watch for `indexed <height> / node <height>`. With no flags the process
   prints `Index synchronized at height <height>.` when it catches up to a
   ready node. Compare the stored tip with the node as shown under Validation.
5. Restart with the same settings and database. Add `--follow` to watch
   for subsequent blocks and mempool changes. Stop with Ctrl+C or send
   SIGTERM on Linux. A SQLite write already in progress can finish before
   shutdown. Restart resumes from the latest committed tip.

## Indexing process

At startup the indexer reads `getblockhash 0`, checks it against the
configured genesis, probes for transaction indexing, and requires
`getblockchaininfo.pruned=false`. It locks the database path, initializes
the schema, checks SQLite integrity and foreign keys, then records or checks
the genesis binding.

Each iteration reads the last committed block and gets the node height and best
hash. An empty database starts at genesis. Otherwise, it requests up to four
successive hashes beyond the committed tip and fetches the corresponding blocks
concurrently. It validates the returned heights and identities, checks parent
links in order, and asks the node for the last height's canonical hash again
before writing. A changed anchor or parent link causes a retry.

One immediate SQLite transaction writes the whole window. The store checks
block continuity, the bound genesis for the first block, transaction ID format,
coinbase position, output positions, script hex, atom amounts, and whether each
spent outpoint exists and is unspent. Database keys enforce uniqueness. A failed
window leaves the previous checkpoint intact, and reapplying the same tip is
idempotent. Valid asset operations update their own tables. Rejected operations
are recorded after reverting that operation's savepoint. Confirmations for
indexed queries use `indexed_tip - block_height + 1`. Mempool entries have
zero confirmations and can change between observations.

If the local tip is above the node or disagrees with its active hash at that
height, synchronization removes one local tip block transactionally and repeats
until it finds the common ancestor, then indexes the replacement branch. Deletes
cascade through transaction and output records, surviving outputs spent by
removed transactions become unspent, and rollback clears the observed mempool.
Protocol-specific rollback routines repair or preserve their journals.
A FATCAT reveal rolled out of the chain leaves its draw journal in place and
halts further draws by design.

In follow mode, transient synchronization errors after startup are reported
and retried after three seconds. One-shot mode exits after three consecutive
synchronization errors. An RPC request can take up to 30 seconds. Startup
validation errors and database write or rollback errors exit the process.
Follow mode refreshes the mempool at the current tip on the three-second
poll cycle.

The checks establish index consistency with the connected node's reported chain.
The indexer trusts Core for active-chain selection, block and transaction
hashes, proof of work, AuxPoW, Merkle commitments, script execution, and
ML-DSA or other witness validation. AuxPoW RPC fields are unused. Raw
transactions, input scripts and witnesses are outside the confirmed ledger
projection. Output script bytes are stored as supplied. The ledger's local
asset rules are application rules.

## Database

SQLite uses WAL, `synchronous=FULL`, foreign keys, and a five-second busy
timeout. The main file and its `-wal` and `-shm` sidecars share a directory.
A separate advisory `.lock` file coordinates indexer writers and can remain
after shutdown. The active lock is held by the running process.

| Tables | Keys, indexes, and relationships |
|---|---|
| `blocks`, `transactions`, `outputs`, `meta` | Blocks are keyed by height with unique hashes. Transactions are keyed by txid and indexed by height and position. Outputs are keyed by (txid, n) and indexed by script and spender, including a partial spender index. `spent_by` references the spending transaction. `meta` stores genesis and ledger version. The highest committed block is the checkpoint. |
| `mempool`, `observed_mempool`, `mempool_inputs`, `mempool_outputs`, `pending` | Mempool membership and application submissions are keyed by txid. Observed inputs and outputs are keyed by (txid, n), with lookups by previous outpoint and output script. |
| `assets`, `collections`, `creation_names`, `collection_nfts`, `asset_mints`, `asset_notes`, `asset_events`, `invalid_asset_operations` | Asset records join to confirmed transactions and outputs. Notes are keyed by outpoint and indexed by asset. Creation names are keyed by kind and normalized name. |
| `map_requests`, `maps`, `map_reveal_audit`, `soqumap_requests`, `soqumap_assignments`, `soqumaps` | Requests reference transactions. Reveals have unique heights and block hashes. The first reveal audit uses (txid, block_hash). Assignments use unique heights. |
| `fatcat_claims`, `fatcat_pool_claims`, `fatcat_requests`, `fatcat_draws` | Claims are keyed by outpoint and indexed by serial. Requests and draws are keyed by txid. The draw journal enforces uniqueness for serials, variants, numbers and metadata hashes. |
| `listings`, `trades`, `listing_receipts`, `listing_submissions` | Listings reference outputs and are indexed by asset. Trades reference confirmed transactions and listings. Receipts are indexed by offer and payer. Submissions link offer/payer pairs to pending transactions. |

Schema creation and migrations are embedded in the Rust source. Startup rejects
a populated unversioned database or an incompatible ledger version. The included
in-place integer-token migration removes legacy decimal-token projections and
their associated listings and events, and drops the decimals column while
preserving base-chain records. Startup also seeds known historical names for
eligible mainnet records. Use SQLite's backup facility or stop the indexer
before making a filesystem backup.

## API reference

The standalone indexer has no HTTP server or public HTTP endpoints.
For local queries, use SQLite or the Rust library. Common library calls
include `Store::tip()`, `Store::block(height)`,
`Store::transaction_location(txid)`, and `Store::history(script_hex)`.
`Store::block` returns height, hash, parent, time, and up to 1,000
transaction IDs. SQL can read a block's full transaction list.

## Testing

Run these commands from the checkout root.

```sh
cargo test --workspace --locked
cargo test --locked -p soq-indexer --test cli
cargo test --locked -p soq-indexer --test ledger
cargo test --locked -p soq-indexer --lib
cargo test --locked -p soq-indexer --lib rpc::tests
cargo test --locked -p soq-indexer reorg
```

The workspace run covers unit and integration tests. The CLI tests exercise
help, configuration validation, and unsupported arguments. The ledger suite
uses decoded block fixtures and covers SQLite initialization, persistence,
spend tracking, mempool projection, asset transitions, restart, and rollback.
The library suite covers atomic failed windows, read snapshots, the token
migration, rollback, RPC response ID ordering, and the txindex probe. These tests
run without a node. The live RPC checks below are manual.

## Validation

Use a synchronized Soqucoin Core node and run the indexer once. Use the
same network and node configuration with `soqucoin-cli` as with the indexer.
For regtest or stagenet include the corresponding network argument to the
node CLI. The following mainnet example assumes `SOQ_DATABASE` is exported
and `jq` and `sqlite3` are installed.

```sh
soqucoin-cli getblockchaininfo | jq '{blocks,bestblockhash,initialblockdownload,pruned}'
sqlite3 -readonly "$SOQ_DATABASE" \
  'SELECT height,hash FROM blocks ORDER BY height DESC LIMIT 1'
sqlite3 -readonly "$SOQ_DATABASE" 'PRAGMA integrity_check; PRAGMA foreign_key_check;'
```

Compare the two tips. If Core advanced in the meantime, compare at the
specific indexed height.

```sh
height=$(sqlite3 -readonly "$SOQ_DATABASE" 'SELECT max(height) FROM blocks')
hash=$(soqucoin-cli getblockhash "$height")
sqlite3 -readonly "$SOQ_DATABASE" "SELECT hash FROM blocks WHERE height=$height"
soqucoin-cli getblock "$hash" 2 | jq -r '.tx[].txid'
sqlite3 -readonly "$SOQ_DATABASE" \
  "SELECT txid FROM transactions WHERE height=$height ORDER BY position"
```

The hashes and ordered txids should match. Compare output position, amount,
and `scriptPubKey.hex` for representative transactions against the
`outputs` rows. SQLite values are integer atoms (100,000,000 per SOQ).
Compare current spend state with `gettxout <txid> <vout> false` on a caught-up

**UTXO comparison note:** When validating with `gettxout`, compare only spendable, non-genesis outputs. The indexer may retain `OP_RETURN` output records even though these outputs are excluded from the node's UTXO set.

node, using non-genesis outputs. After restarting the indexer with the same
file, verify the earlier checkpoint and rerun the tip comparison.

For a live reorganization check, use a disposable regtest chain and database.
Index several blocks, invalidate a recent block using the node's
`invalidateblock <hash>`, then run the indexer again. Verify that the old
branch's height/hash pairs and transactions disappear, an output spent only
on that branch becomes unspent again, and the SQLite integrity and foreign
key checks succeed. Extend the replacement branch and compare tips again.
To check disconnect recovery, stop the node after follow mode has reached
the tip, restart Core with the same settings, and check that progress resumes.

## Troubleshooting

- **Connection refused or timed out.** Check that Core is running with RPC
  enabled and that its bind and port match `SOQ_RPC_URL`. Use a numeric
  loopback IP. Check Core logs if it is loading the block index.
- **Authentication or invalid RPC response.** Verify both credential variables
  against the node configuration. HTTP errors with non-JSON bodies can appear
  as invalid RPC responses. Check the endpoint with the node CLI.
- **Wrong genesis, pruned node, or failed txindex probe.** Check the chain
  selection, enable the full transaction index, and use an unpruned node.
  A node still in initial block download may serve only a partial chain.
- **Database lock or permission error.** Create the parent directory, grant
  the indexer account write access to it, and stop any other indexer writer
  for that database. Keep the lock file in place while a writer is running.
- **Interrupted sync or a rollback report.** Restart using the same database.
  Compare its checkpoint with Core. Repeated rollbacks warrant checking
  whether Core itself is switching branches or serving inconsistent responses.
- **Corruption, chain binding mismatch, or incompatible schema.** Preserve
  a copy of the database and its WAL for investigation. Use a new database
  path for a rebuild from the verified node, or restore a known-good backup.

## Security

Use a dedicated local service account and restrict access to its environment
and database directory. Keep RPC credentials out of source control, terminal
history, and logs. Use loopback RPC bindings on the node and a secure local
tunnel if the node runs on another host. HTTP Basic authentication by itself
does not encrypt traffic. The indexer opens no listening socket.

Treat the connected node as trusted. Coherent but incorrect RPC data can
produce an incorrect index. Restrict write access to the index and avoid
sharing it between writers outside the indexer's locking mechanism. The
library exposes additional node and database operations for embedding; the
standalone synchronization process uses read-only node RPC methods.

## Current limitations

- Confirmed inputs are represented by spend links on previous outputs. Mempool
  inputs have a separate table. Address-related queries are based on exact
  script matches and supported witness-script conversion.
- Full block headers, AuxPoW proofs, witness data, raw confirmed transactions,
  and a general address registry are outside the persistent projection.
  The node remains the source for those details.
- A rebuild requires scanning from genesis. Keep a backup when preserving
  application-specific records like pending submissions or reveal journals
  matters.
- Reorganization recovery removes one block per iteration. The index and
  mempool are point-in-time snapshots that may lag the node between polls.

## License

The indexer, wallet and supporting first-party code are distributed under the
[MIT License](LICENSE), with the original copyright notice preserved. The
bundled SQLite and cryptographic dependencies retain their upstream terms. See
[NOTICE](NOTICE) and [THIRD_PARTY_LICENSES.txt](THIRD_PARTY_LICENSES.txt) for
provenance, attribution, and dependency license texts.
