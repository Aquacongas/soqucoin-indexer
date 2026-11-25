use anyhow::{ensure, Context, Result};
use serde_json::json;
use soq_indexer::{next_window, Rpc, Store, Sync};
use std::{env, fs::OpenOptions, path::Path, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let follow = match args.next() {
        None => false,
        Some(flag) if flag == "--follow" && args.next().is_none() => true,
        Some(flag) if flag == "--help" && args.next().is_none() => {
            println!(
                "Usage: soq-indexer [--follow | --help]

Synchronize a local SQLite index from genesis or its committed tip.
With no flags, exit when caught up to a node that has completed initial download.

  --follow  Continue indexing new blocks and observe the mempool.
  --help    Print this help and exit.

Environment:
  SOQ_RPC_URL       Numeric loopback HTTP URL (default: http://127.0.0.1:9999)
  SOQ_RPC_USER      RPC Basic authentication username (required)
  SOQ_RPC_PASSWORD  RPC Basic authentication password (required)
  SOQ_DATABASE      SQLite file in an existing directory (default: soq-index.sqlite)
  SOQ_NETWORK       main, regtest, or stagenet (default: main)
  SOQ_GENESIS       Expected genesis hash (default: selected network's genesis)"
            );
            return Ok(());
        }
        _ => anyhow::bail!("usage: soq-indexer [--follow | --help]"),
    };

    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let stop = async {
        #[cfg(unix)]
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await
    };
    tokio::select! {
        biased;
        result = stop => result.context("shutdown signal failed"),
        result = synchronize(follow) => result,
    }
}

async fn synchronize(follow: bool) -> Result<()> {
    let network = env::var("SOQ_NETWORK").unwrap_or_else(|_| "main".into());
    let expected_genesis = match network.as_str() {
        "main" => "0d828600816cbd7c23789660b53f90cb6ec7ff85540698e13845eb2d2f0486a8",
        "regtest" => "3d2160a3b5dc4a9d62e7e66a295f70313ac808440ef7400d6c0772171ce973a5",
        "stagenet" => "97df3ae79eaf5623c0feecfa1079439f8acdfea06a0f2acb4ef63c6b9ad91bb0",
        _ => anyhow::bail!("unsupported network"),
    };
    let expected_genesis = env::var("SOQ_GENESIS").unwrap_or_else(|_| expected_genesis.into());
    let rpc = Rpc::new(
        env::var("SOQ_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:9999".into()),
        env::var("SOQ_RPC_USER").context("set SOQ_RPC_USER")?,
        env::var("SOQ_RPC_PASSWORD").context("set SOQ_RPC_PASSWORD")?,
    )?;
    let genesis = rpc.hash(0).await?;
    ensure!(
        genesis == expected_genesis,
        "node genesis differs from configured chain"
    );
    rpc.require_txindex().await?;
    let info = rpc.call("getblockchaininfo", json!([])).await?;
    ensure!(
        info["pruned"] == false,
        "an unpruned Soqucoin node is required"
    );

    let database = env::var("SOQ_DATABASE").unwrap_or_else(|_| "soq-index.sqlite".into());
    let path = Path::new(&database);
    let canonical = if path.exists() {
        path.canonicalize()?
    } else {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        parent.canonicalize()?.join(
            path.file_name()
                .context("index database filename missing")?,
        )
    };
    let mut lock_path = canonical.into_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    lock.try_lock()
        .context("the index database is already in use")?;
    let mut store = Store::open(path)?;
    store.verify()?;
    store.bind_chain(&genesis)?;

    let mut failures = 0;
    loop {
        let tip = store.tip()?;
        let window = next_window(&rpc, tip).await;
        let delay = match window {
            Ok(Sync::Blocks {
                blocks,
                node_height,
            }) => {
                tokio::task::block_in_place(|| store.apply_window(&blocks))?;
                let height = store.tip()?.context("committed block tip missing")?.0;
                println!("indexed {height} / node {node_height}");
                Duration::ZERO
            }
            Ok(Sync::Rollback(height)) => {
                tokio::task::block_in_place(|| store.rollback(height))?;
                eprintln!("reorganization: rolled back from height {height}");
                Duration::from_millis(100)
            }
            Ok(Sync::Current {
                node_height,
                node_ready,
            }) => {
                if !follow && node_ready {
                    println!("Index synchronized at height {node_height}.");
                    return Ok(());
                }
                if follow {
                    if let Err(error) = refresh_mempool(&rpc, &mut store).await {
                        eprintln!("mempool refresh: {error:#}");
                    }
                }
                Duration::from_secs(3)
            }
            Ok(Sync::Retry) => Duration::from_millis(100),
            Err(error) => {
                failures += 1;
                if !follow && failures >= 3 {
                    return Err(error.context("index synchronization failed"));
                }
                eprintln!("index synchronization: {error:#}");
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
        };
        failures = 0;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

async fn refresh_mempool(rpc: &Rpc, store: &mut Store) -> Result<()> {
    let ids: Vec<String> = serde_json::from_value(rpc.call("getrawmempool", json!([])).await?)?;
    let known = store.known_mempool()?;
    let mut decoded = Vec::new();
    for id in ids.iter().filter(|id| !known.contains(*id)) {
        match rpc.call("getrawtransaction", json!([id, true])).await {
            Ok(body) if body["txid"] == *id && body["blockhash"].is_null() => decoded.push(body),
            Ok(_) => continue,
            Err(error) => {
                let current: Vec<String> =
                    serde_json::from_value(rpc.call("getrawmempool", json!([])).await?)?;
                if current.contains(id) {
                    return Err(error);
                }
            }
        }
    }
    tokio::task::block_in_place(|| store.observe_mempool_transactions(&ids, &decoded))
}
