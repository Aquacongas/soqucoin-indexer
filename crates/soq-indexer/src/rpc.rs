use anyhow::{ensure, Context, Result};
use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::{self, SendRequest};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, task::JoinHandle};

const CONNECTIONS: usize = 8;
const BACKGROUND_CONNECTIONS: usize = 4;
const MAX_RESPONSE: usize = 64 * 1024 * 1024;
const DETAIL_CACHE_BYTES: usize = 8 * 1024 * 1024;
const DETAIL_CACHE_ENTRIES: usize = 128;

struct Connection {
    sender: SendRequest<Full<Bytes>>,
    driver: JoinHandle<()>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

struct Detail {
    id: String,
    hash: String,
    body: Bytes,
}

struct Shared {
    authority: String,
    auth: String,
    permits: Semaphore,
    idle: Mutex<Vec<Connection>>,
    details: Mutex<VecDeque<Detail>>,
}

/// RPC access to an authenticated loopback node.
#[derive(Clone)]
pub struct Rpc {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Rejection(Value);

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "node rejected RPC: {}", self.0)
    }
}

impl std::error::Error for Rejection {}

impl Rpc {
    pub fn new(url: String, user: String, password: String) -> Result<Self> {
        let authority = url
            .strip_prefix("http://")
            .context("RPC requires loopback HTTP")?
            .trim_end_matches('/')
            .to_owned();
        let addr: std::net::SocketAddr = authority
            .parse()
            .context("RPC requires numeric loopback IP and port")?;
        ensure!(
            addr.ip().is_loopback(),
            "RPC must use loopback; use a secure local tunnel for remote nodes"
        );
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        Ok(Self {
            shared: Arc::new(Shared {
                authority,
                auth: format!("Basic {auth}"),
                permits: Semaphore::new(CONNECTIONS),
                idle: Mutex::new(Vec::new()),
                details: Mutex::new(VecDeque::new()),
            }),
        })
    }

    pub fn separate_pool(&self) -> Self {
        Self {
            shared: Arc::new(Shared {
                authority: self.shared.authority.clone(),
                auth: self.shared.auth.clone(),
                permits: Semaphore::new(BACKGROUND_CONNECTIONS),
                idle: Mutex::new(Vec::new()),
                details: Mutex::new(VecDeque::new()),
            }),
        }
    }

    async fn connect(&self) -> Result<Connection> {
        let socket = tokio::net::TcpStream::connect(&self.shared.authority)
            .await
            .context("RPC connection failed")?;
        let (sender, connection) = http1::handshake(hyper_util::rt::TokioIo::new(socket)).await?;
        let driver = tokio::spawn(async move {
            let _ = connection.await;
        });
        Ok(Connection { sender, driver })
    }

    async fn request(&self, data: Value) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let _permit = self.shared.permits.acquire().await?;
            let mut connection = loop {
                let idle = self.shared.idle.lock().expect("RPC pool lock").pop();
                match idle {
                    Some(mut connection) if !connection.driver.is_finished() => {
                        if connection.sender.ready().await.is_ok() {
                            break connection;
                        }
                    }
                    Some(_) => {}
                    None => break self.connect().await?,
                }
            };
            let request = hyper::Request::post("/")
                .header("host", &self.shared.authority)
                .header("authorization", &self.shared.auth)
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(serde_json::to_vec(&data)?)))?;
            let mut response = connection.sender.send_request(request).await?;
            let mut body = Vec::new();
            while let Some(frame) = response.body_mut().frame().await {
                if let Ok(data) = frame?.into_data() {
                    ensure!(
                        body.len() + data.len() <= MAX_RESPONSE,
                        "RPC response too large"
                    );
                    body.extend(data);
                }
            }
            if !connection.driver.is_finished() && !connection.sender.is_closed() {
                let mut idle = self.shared.idle.lock().expect("RPC pool lock");
                idle.retain(|connection| {
                    !connection.driver.is_finished() && !connection.sender.is_closed()
                });
                if idle.len() < CONNECTIONS {
                    idle.push(connection);
                }
            }
            serde_json::from_slice(&body).context("invalid RPC response")
        })
        .await
        .context("RPC timeout")?
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        ensure!(
            [
                "getblockchaininfo",
                "getnetworkinfo",
                "getmempoolinfo",
                "getblockhash",
                "getblock",
                "getrawmempool",
                "getrawtransaction",
                "gettxout",
                "estimatesmartfee",
                "estimatefee",
                "sendrawtransaction",
                "decoderawtransaction",
            ]
            .contains(&method),
            "RPC method not allowed"
        );
        let reply = self
            .request(json!({"jsonrpc":"1.0","id":"soq-indexer","method":method,"params":params}))
            .await?;
        ensure!(reply["id"] == "soq-indexer", "RPC response ID mismatch");
        Self::result(reply)
    }

    fn result(mut reply: Value) -> Result<Value> {
        ensure!(reply.get("error").is_some(), "RPC error field missing");
        if !reply["error"].is_null() {
            return Err(Rejection(reply["error"].take()).into());
        }
        Ok(reply
            .get_mut("result")
            .context("RPC result missing")?
            .take())
    }

    pub async fn hash(&self, height: u64) -> Result<String> {
        Self::parse_hash(self.call("getblockhash", json!([height])).await?)
    }

    fn parse_hash(value: Value) -> Result<String> {
        let hash = value.as_str().context("invalid block hash")?;
        soq_core::hash_bytes(hash)?;
        Ok(hash.to_owned())
    }

    pub(crate) async fn hashes(&self, first: u64, last: u64) -> Result<Vec<String>> {
        if first == last {
            return Ok(vec![self.hash(first).await?]);
        }
        let requests: Vec<_> = (first..=last)
            .map(|height| json!({"jsonrpc":"1.0","id":height,"method":"getblockhash","params":[height]}))
            .collect();
        Self::decode_hash_batch(first, last, self.request(Value::Array(requests)).await?)
    }

    fn decode_hash_batch(first: u64, last: u64, response: Value) -> Result<Vec<String>> {
        let count = (last - first + 1) as usize;
        let Value::Array(replies) = response else {
            anyhow::bail!("invalid block hash batch response");
        };
        ensure!(replies.len() == count, "incomplete block hash batch");
        let mut hashes = vec![None; count];
        for reply in replies {
            let height = reply["id"]
                .as_u64()
                .context("invalid block hash request ID")?;
            ensure!(
                (first..=last).contains(&height),
                "unexpected block hash request ID"
            );
            let slot = &mut hashes[(height - first) as usize];
            ensure!(slot.is_none(), "duplicate block hash request ID");
            *slot = Some(Self::parse_hash(Self::result(reply)?)?);
        }
        hashes
            .into_iter()
            .map(|hash| hash.context("missing block hash"))
            .collect()
    }

    pub async fn require_txindex(&self) -> Result<()> {
        // This Core release can find unspent transactions without txindex. Its
        // error for an absent txid distinguishes the configured index modes.
        let probe = self
            .call("getrawtransaction", json!(["ff".repeat(32), false]))
            .await;
        Self::check_txindex_probe(probe)
    }

    fn check_txindex_probe(probe: Result<Value>) -> Result<()> {
        if let Err(error) = &probe {
            if let Some(rejection) = error.downcast_ref::<Rejection>() {
                let message = rejection.0["message"].as_str().unwrap_or_default();
                if rejection.0["code"] == -5
                    && message.starts_with("No such mempool or blockchain transaction")
                {
                    return Ok(());
                }
                if rejection.0["code"] == -5 && message.contains("Use -txindex") {
                    anyhow::bail!("Soqucoin Core requires txindex=1; rebuild the node index before starting the indexer");
                }
            }
        }
        probe.context("could not verify Soqucoin Core txindex=1")?;
        anyhow::bail!("unexpected response to Soqucoin Core txindex probe")
    }

    pub async fn transaction_detail(&self, id: &str, height: u64, hash: &str) -> Result<Value> {
        let cached = {
            let mut details = self.shared.details.lock().expect("transaction cache lock");
            details
                .iter()
                .position(|entry| entry.id == id && entry.hash == hash)
                .map(|index| {
                    let entry = details.remove(index).expect("cache entry");
                    let body = entry.body.clone();
                    details.push_front(entry);
                    body
                })
        };
        if let Some(body) = cached {
            return Ok(serde_json::from_slice(&body)?);
        }

        let mut detail = if height == 0 {
            // Core may exclude the genesis coinbase from its transaction index.
            let mut block = self.call("getblock", json!([hash, 2])).await?;
            ensure!(
                block["height"] == 0 && block["hash"] == hash,
                "invalid genesis block response"
            );
            block["tx"]
                .as_array_mut()
                .and_then(|txs| txs.iter_mut().find(|tx| tx["txid"] == id))
                .context("transaction missing from genesis block")?
                .take()
        } else {
            let detail = self.call("getrawtransaction", json!([id, true])).await?;
            ensure!(
                detail["blockhash"] == hash
                    && detail["confirmations"].as_u64().is_some_and(|n| n > 0),
                "transaction is no longer in its indexed block"
            );
            detail
        };
        ensure!(detail["txid"] == id, "transaction response ID mismatch");
        let fields = detail
            .as_object_mut()
            .context("invalid transaction details")?;
        for field in ["hex", "blockhash", "confirmations", "time", "blocktime"] {
            fields.remove(field);
        }
        let body = Bytes::from(serde_json::to_vec(&detail)?);
        if body.len() <= DETAIL_CACHE_BYTES {
            let mut details = self.shared.details.lock().expect("transaction cache lock");
            details.retain(|entry| entry.id != id || entry.hash != hash);
            let mut used: usize = details.iter().map(|entry| entry.body.len()).sum();
            while details.len() >= DETAIL_CACHE_ENTRIES || used + body.len() > DETAIL_CACHE_BYTES {
                used -= details.pop_back().expect("cache entry").body.len();
            }
            details.push_front(Detail {
                id: id.to_owned(),
                hash: hash.to_owned(),
                body,
            });
        }
        Ok(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_id_order_and_completeness() {
        let one = "01".repeat(32);
        let two = "02".repeat(32);
        let higher = json!({"id":11,"result":two,"error":null});
        let lower = json!({"id":10,"result":one,"error":null});
        assert_eq!(
            Rpc::decode_hash_batch(10, 11, json!([higher, lower])).unwrap(),
            [one, two]
        );
        for invalid in [
            json!([higher, higher]),
            json!([lower]),
            json!([lower, {"id":12}]),
        ] {
            assert!(Rpc::decode_hash_batch(10, 11, invalid).is_err());
        }
    }

    #[test]
    fn txindex_probe_distinguishes_node_configuration() {
        let reject = |code, message| Err(Rejection(json!({"code":code,"message":message})).into());
        assert!(
            Rpc::check_txindex_probe(reject(-5, "No such mempool or blockchain transaction"))
                .is_ok()
        );
        assert!(Rpc::check_txindex_probe(reject(
            -5,
            "No such mempool transaction. Use -txindex to enable blockchain transaction queries"
        ))
        .unwrap_err()
        .to_string()
        .contains("requires txindex=1"));
        assert!(Rpc::check_txindex_probe(reject(-28, "Loading block index")).is_err());
        assert!(Rpc::check_txindex_probe(Ok(json!("unexpected"))).is_err());
    }
}
