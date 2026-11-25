//! SQMK v1 binary data carrier. All amounts are integer base units.
pub const SQR20_VERSION: u8 = 2;
pub const TOKEN_OUTPUT_ATOMS: u64 = 1_000_000;
pub const FATCAT_SUPPLY: u16 = 1006;
pub const FATCAT_PRICE: u64 = 2_000 * crate::fees::ATOMS_PER_SOQ;
pub const FATCAT_ATLAS: &str = "df4769c2823249ae11cec99b559c47fe86ef954876d62f38f36f0b5d4dfad1ee";
pub const SOQ721_VERSION: u8 = 1;
pub const SQR20_NAME: &str = "SQR-20";
pub const SOQ721_NAME: &str = "SOQ-721";
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
pub fn name_key(name: &str) -> Result<[u8; 16]> {
    ensure!(
        !name.trim().is_empty() && name.chars().count() <= 100,
        "name must have 1 to 100 characters"
    );
    let folded = unicase::UniCase::new(name.trim()).to_folded_case();
    let hash = crate::sha256(folded.as_bytes());
    Ok(hash[..16].try_into().expect("fixed hash prefix"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Nft {
        metadata: [u8; 32],
        destination: u8,
    },
    Deploy {
        metadata: [u8; 32],
        cap: u64,
        limit: u64,
        ticker: [u8; 8],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name_key: Option<[u8; 16]>,
    },
    Mint {
        token: [u8; 32],
        amount: u64,
        destination: u8,
    },
    Transfer {
        asset: [u8; 32],
        allocations: Vec<(u8, u64)>,
    },
    Listing {
        offer: [u8; 32],
    },
    Settle {
        asset: [u8; 32],
        quantity: u64,
        price: u64,
    },
    Cancel {
        asset: [u8; 32],
        quantity: u64,
    },
    MapRequest {
        destination: u8,
    },
    FatcatClaim {
        serial: u16,
        destination: u8,
    },
    FatcatPoolClaim {
        first: u16,
        count: u8,
    },
    FatcatPoolMint {
        destination: u8,
    },
    FatcatRequest { destination: u8 },
    FatcatMint {
        destination: u8,
    },
    SoquMapRequest {
        destination: u8,
    },
    Collection {
        metadata: [u8; 32],
        maximum: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name_key: Option<[u8; 16]>,
    },
    CollectionNft {
        collection: [u8; 32],
        metadata: [u8; 32],
        destination: u8,
    },
}
impl Operation {
    pub const MINT_CONFIRMATIONS: u64 = 6;

    pub fn funding_confirmations(&self) -> u64 {
        match self {
            Self::Transfer { .. }
            | Self::Settle { .. }
            | Self::Cancel { .. }
            | Self::Listing { .. } => 1,
            _ => Self::MINT_CONFIRMATIONS,
        }
    }

    pub fn platform_kind(&self) -> Option<crate::fees::MintKind> {
        use crate::fees::MintKind;
        match self {
            Self::Nft { .. } | Self::CollectionNft { .. } => Some(MintKind::Nft),
            Self::Deploy { .. } => Some(MintKind::TokenDeploy),
            Self::Mint { .. } => Some(MintKind::TokenMint),
            Self::Collection { .. } => Some(MintKind::Collection),
            Self::Listing { .. } => Some(MintKind::Listing),
            Self::FatcatRequest { .. } | Self::FatcatMint { .. } | Self::FatcatPoolMint { .. } => Some(MintKind::Fatcat),
            Self::MapRequest { .. } | Self::SoquMapRequest { .. } => Some(MintKind::Map),
            _ => None,
        }
    }
    pub fn script(&self) -> Result<Vec<u8>> {
        let mut b = b"SQMK\x01".to_vec();
        match self {
            Self::Nft {
                metadata,
                destination,
            } => {
                b.push(1);
                b.extend(metadata);
                b.push(*destination);
            }
            Self::Deploy {
                metadata,
                cap,
                limit,
                ticker,
                name_key,
            } => {
                b.push(2);
                b.extend(metadata);
                b.extend(cap.to_le_bytes());
                b.extend(limit.to_le_bytes());
                b.extend(ticker);
                if let Some(key) = name_key {
                    b.extend(key);
                }
            }
            Self::Mint {
                token,
                amount,
                destination,
            } => {
                b.push(3);
                b.extend(token);
                b.extend(amount.to_le_bytes());
                b.push(*destination);
            }
            Self::Transfer { asset, allocations } => {
                ensure!(
                    !allocations.is_empty() && allocations.len() <= 4,
                    "1 to 4 allocations required"
                );
                b.push(4);
                b.extend(asset);
                b.push(allocations.len() as u8);
                for (n, q) in allocations {
                    b.push(*n);
                    b.extend(q.to_le_bytes());
                }
            }
            Self::Listing { offer } => {
                b.push(9);
                b.extend(offer);
            }
            Self::Settle {
                asset,
                quantity,
                price,
            } => {
                b.push(10);
                b.extend(asset);
                b.extend(quantity.to_le_bytes());
                b.extend(price.to_le_bytes());
            }
            Self::Cancel { asset, quantity } => {
                b.push(11);
                b.extend(asset);
                b.extend(quantity.to_le_bytes());
            }
            Self::MapRequest { destination } => {
                b.extend([5, *destination]);
            }
            Self::SoquMapRequest { destination } => b.extend([8, *destination]),
            Self::FatcatClaim {
                serial,
                destination,
            } => {
                ensure!(
                    (1..=FATCAT_SUPPLY).contains(serial),
                    "invalid FATCAT claim serial"
                );
                b.push(12);
                b.extend(serial.to_le_bytes());
                b.push(*destination);
            }
            Self::FatcatRequest { destination } => b.extend([16, *destination]),
            Self::FatcatMint { destination } => b.extend([13, *destination]),
            Self::FatcatPoolMint { destination } => b.extend([14, *destination]),
            Self::FatcatPoolClaim { first, count } => {
                ensure!(
                    *first > 0
                        && (1..=64).contains(count)
                        && u32::from(*first) + u32::from(*count) - 1 <= u32::from(FATCAT_SUPPLY),
                    "invalid FATCAT pool range"
                );
                b.push(15);
                b.extend(first.to_le_bytes());
                b.push(*count);
            }
            Self::Collection {
                metadata,
                maximum,
                name_key,
            } => {
                b.push(6);
                b.extend(metadata);
                b.extend(maximum.to_le_bytes());
                if let Some(key) = name_key {
                    b.extend(key);
                }
            }
            Self::CollectionNft {
                collection,
                metadata,
                destination,
            } => {
                b.push(7);
                b.extend(collection);
                b.extend(metadata);
                b.push(*destination);
            }
        }
        ensure!(b.len() <= 80, "data carrier limit");
        let mut s = vec![0x6a];
        if b.len() > 75 {
            s.push(0x4c);
        }
        s.push(b.len() as u8);
        s.extend(b);
        Ok(s)
    }
    pub fn parse(script: &[u8]) -> Result<Self> {
        ensure!(script.first() == Some(&0x6a), "not data carrier");
        let b = if script.get(1) == Some(&0x4c) {
            let n = *script.get(2).context("push length")? as usize;
            ensure!(
                n > 75 && n <= 80 && script.len() == n + 3,
                "noncanonical push"
            );
            &script[3..]
        } else {
            let n = *script.get(1).context("push length")? as usize;
            ensure!(n <= 75 && script.len() == n + 2, "noncanonical push");
            &script[2..]
        };
        ensure!(b.starts_with(b"SQMK\x01"), "unknown protocol version");
        Ok(match b.get(5) {
            Some(1) => {
                ensure!(b.len() == 39, "NFT length");
                Self::Nft {
                    metadata: b[6..38].try_into()?,
                    destination: b[38],
                }
            }
            Some(2) => {
                ensure!(matches!(b.len(), 62 | 78), "deploy length");
                Self::Deploy {
                    metadata: b[6..38].try_into()?,
                    cap: u64::from_le_bytes(b[38..46].try_into()?),
                    limit: u64::from_le_bytes(b[46..54].try_into()?),
                    ticker: b[54..62].try_into()?,
                    name_key: b.get(62..78).map(|key| key.try_into()).transpose()?,
                }
            }
            Some(3) => {
                ensure!(b.len() == 47, "mint length");
                Self::Mint {
                    token: b[6..38].try_into()?,
                    amount: u64::from_le_bytes(b[38..46].try_into()?),
                    destination: b[46],
                }
            }
            Some(4) => {
                ensure!(b.len() >= 39, "transfer length");
                let n = b[38] as usize;
                ensure!(
                    (1..=4).contains(&n) && b.len() == 39 + n * 9,
                    "transfer length"
                );
                Self::Transfer {
                    asset: b[6..38].try_into()?,
                    allocations: b[39..]
                        .chunks_exact(9)
                        .map(|a| {
                            (
                                a[0],
                                u64::from_le_bytes(a[1..9].try_into().expect("allocation size")),
                            )
                        })
                        .collect(),
                }
            }
            Some(5) => {
                ensure!(b.len() == 7, "map request length");
                Self::MapRequest { destination: b[6] }
            }
            Some(6) => {
                ensure!(matches!(b.len(), 42 | 58), "collection length");
                Self::Collection {
                    metadata: b[6..38].try_into()?,
                    maximum: u32::from_le_bytes(b[38..42].try_into()?),
                    name_key: b.get(42..58).map(|key| key.try_into()).transpose()?,
                }
            }
            Some(7) => {
                ensure!(b.len() == 71, "collection NFT length");
                Self::CollectionNft {
                    collection: b[6..38].try_into()?,
                    metadata: b[38..70].try_into()?,
                    destination: b[70],
                }
            }
            Some(8) => {
                ensure!(b.len() == 7, "SOQ Map request length");
                Self::SoquMapRequest { destination: b[6] }
            }
            Some(9) => {
                ensure!(b.len() == 38, "listing receipt length");
                Self::Listing {
                    offer: b[6..38].try_into()?,
                }
            }
            Some(10) => {
                ensure!(b.len() == 54, "settlement length");
                Self::Settle {
                    asset: b[6..38].try_into()?,
                    quantity: u64::from_le_bytes(b[38..46].try_into()?),
                    price: u64::from_le_bytes(b[46..54].try_into()?),
                }
            }
            Some(11) => {
                ensure!(b.len() == 46, "cancellation length");
                Self::Cancel {
                    asset: b[6..38].try_into()?,
                    quantity: u64::from_le_bytes(b[38..46].try_into()?),
                }
            }
            Some(12) => {
                ensure!(b.len() == 9, "FATCAT claim length");
                let serial = u16::from_le_bytes(b[6..8].try_into()?);
                ensure!(
                    (1..=FATCAT_SUPPLY).contains(&serial),
                    "invalid FATCAT claim serial"
                );
                Self::FatcatClaim {
                    serial,
                    destination: b[8],
                }
            }
            Some(13) => {
                ensure!(b.len() == 7, "FATCAT mint length");
                Self::FatcatMint { destination: b[6] }
            }
            Some(14) => {
                ensure!(b.len() == 7, "FATCAT pool mint length");
                Self::FatcatPoolMint { destination: b[6] }
            }
            Some(15) => {
                ensure!(b.len() == 9, "FATCAT pool issue length");
                let first = u16::from_le_bytes(b[6..8].try_into()?);
                let count = b[8];
                ensure!(
                    first > 0
                        && (1..=64).contains(&count)
                        && u32::from(first) + u32::from(count) - 1 <= u32::from(FATCAT_SUPPLY),
                    "invalid FATCAT pool range"
                );
                Self::FatcatPoolClaim { first, count }
            }
            Some(16) => {
                ensure!(b.len() == 7, "FATCAT request length");
                Self::FatcatRequest { destination: b[6] }
            }
            _ => anyhow::bail!("unknown operation"),
        })
    }
}
pub fn operation(outputs: &[crate::Output]) -> Result<Operation> {
    let carriers: Vec<_> = outputs
        .iter()
        .filter(|o| o.script.first() == Some(&0x6a))
        .collect();
    ensure!(carriers.len() == 1, "exactly one data carrier required");
    Operation::parse(&carriers[0].script)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deployment_rejects_obsolete_precision_byte() {
        let deploy = Operation::Deploy {
            metadata: [1; 32],
            cap: 100,
            limit: 10,
            ticker: *b"TEST\0\0\0\0",
            name_key: None,
        };
        let mut obsolete = deploy.script().unwrap();
        obsolete[1] += 1;
        obsolete.insert(2 + 54, 0);
        assert!(Operation::parse(&obsolete).is_err());
    }

    #[test]
    fn all_operations_roundtrip() {
        let ops = [
            Operation::Nft {
                metadata: [7; 32],
                destination: 0,
            },
            Operation::Deploy {
                metadata: [2; 32],
                cap: 100,
                limit: 10,
                ticker: *b"TEST\0\0\0\0",
                name_key: Some(name_key("Example Token").unwrap()),
            },
            Operation::Mint {
                token: [3; 32],
                amount: 9,
                destination: 2,
            },
            Operation::Transfer {
                asset: [5; 32],
                allocations: vec![(0, 3), (1, 4), (2, 5), (3, 6)],
            },
            Operation::MapRequest { destination: 0 },
            Operation::Collection {
                metadata: [2; 32],
                maximum: 100,
                name_key: Some(name_key("Example Collection").unwrap()),
            },
            Operation::CollectionNft {
                collection: [3; 32],
                metadata: [2; 32],
                destination: 1,
            },
        ];
        for op in ops {
            let s = op.script().unwrap();
            assert!(s.len() <= 83);
            assert_eq!(Operation::parse(&s).unwrap(), op);
            for i in 0..s.len() {
                assert!(Operation::parse(&s[..i]).is_err());
            }
        }
    }
}
