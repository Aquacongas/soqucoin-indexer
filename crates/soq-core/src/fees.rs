//! Mandatory platform fees, separate from miner fees. Versioned protocol policy.
use crate::{address_script, Output};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
pub const FEE_ADDRESS: &str = "sq1peq2hd53huwgmxy0ezhj5nzrufvxqax3mtdr752qrfmsnvfw6vzmqe0vwjs";
pub const ATOMS_PER_SOQ: u64 = 100_000_000;
pub const TRANSFER_FEE: u64 = ATOMS_PER_SOQ;
pub const LISTING_FEE: u64 = ATOMS_PER_SOQ;
pub const PURCHASE_FEE: u64 = ATOMS_PER_SOQ;
pub const DEX_SIDE_BPS: u64 = 50;

pub const DISCOUNT_NFTS: [(&str, u8); 3] = [
    (
        "f2f8ae134a5a68f6e62815134b4c2890dc2c068e30712c8c7d14a8bc5bf64817",
        75,
    ),
    (
        "eeb9fa1c6c7019b04a4429a30f6f6a969f98e983fda196ca348399bdb4a63186",
        50,
    ),
    (
        "3c598a90b52038ff4f7432ebee970bb5b48e48da11706b854d716c123f0ed772",
        25,
    ),
];

pub fn discounted_fee(original: u64, discount: u8) -> Result<u64> {
    ensure!(
        matches!(discount, 0 | 25 | 50 | 75),
        "invalid platform discount"
    );
    Ok((u128::from(original) * u128::from(100 - discount)).div_ceil(100) as u64)
}

pub fn authorized_fee(paid: u64, original: u64, discount: u8) -> bool {
    [0, 25, 50, 75]
        .into_iter()
        .take_while(|tier| *tier <= discount)
        .any(|tier| discounted_fee(original, tier).is_ok_and(|fee| fee == paid))
}

pub fn carrier_payment(outputs: &[Output]) -> Result<u64> {
    let index = outputs
        .iter()
        .position(|out| out.script.first() == Some(&0x6a))
        .context("asset fee requires a carrier")?
        + 1;
    Ok(outputs
        .get(index)
        .context("platform payment missing")?
        .value)
}

pub fn dex_commission(price: u64) -> Result<u64> {
    let fee = (u128::from(price) * u128::from(DEX_SIDE_BPS)).div_ceil(10_000) as u64;
    ensure!(price > fee, "trade price is too small for commission");
    Ok(fee)
}
pub fn payment(value: u64) -> Result<Output> {
    Ok(Output {
        value,
        script: address_script(FEE_ADDRESS, "sq")?,
    })
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MintKind {
    Nft,
    Fatcat,
    TokenDeploy,
    TokenMint,
    Collection,
    Listing,
    Map,
}
impl MintKind {
    pub fn fee(self) -> u64 {
        (match self {
            Self::Nft => 5000,
            Self::Fatcat => return crate::protocol::FATCAT_PRICE,
            Self::TokenDeploy => 10000,
            Self::Map => 7500,
            Self::TokenMint | Self::Listing => 1,
            Self::Collection => 1000,
        }) * ATOMS_PER_SOQ
    }
}
pub fn fee_output(kind: MintKind) -> Result<Output> {
    Ok(Output {
        value: kind.fee(),
        script: address_script(FEE_ADDRESS, "sq")?,
    })
}
pub fn verify_fee(outputs: &[Output], kind: MintKind) -> Result<()> {
    verify_exact_payment(outputs, kind.fee(), None)
}
pub fn transfer_output() -> Result<Output> {
    payment(TRANSFER_FEE)
}
pub fn verify_exact_payment(
    outputs: &[Output],
    amount: u64,
    principal: Option<usize>,
) -> Result<()> {
    let expected = payment(amount)?;
    let mut payments = outputs
        .iter()
        .enumerate()
        .filter(|(index, output)| Some(*index) != principal && output.script == expected.script);
    if amount == 0 {
        ensure!(payments.next().is_none(), "unexpected platform fee");
    } else {
        ensure!(
            payments
                .next()
                .is_some_and(|(_, output)| *output == expected)
                && payments.next().is_none(),
            "missing, duplicate, or incorrect platform fee"
        );
    }
    Ok(())
}
pub fn verify_transfer_fee(outputs: &[Output]) -> Result<()> {
    verify_carrier_fee(outputs, TRANSFER_FEE, None)
}
pub fn verify_carrier_fee(outputs: &[Output], amount: u64, principal: Option<usize>) -> Result<()> {
    let index = outputs
        .iter()
        .position(|o| o.script.first() == Some(&0x6a))
        .context("asset fee requires a carrier")?
        + 1;
    let expected = payment(amount)?;
    ensure!(
        outputs.get(index) == Some(&expected),
        "incorrect platform fee output"
    );
    verify_exact_payment(outputs, amount, principal)
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Breakdown {
    pub platform_atoms: u64,
    pub network_atoms: u64,
    pub asset_output_atoms: u64,
    pub total_atoms: u64,
    pub platform_recipient: String,
}
pub fn breakdown(kind: MintKind, network: u64, asset_output: u64) -> Result<Breakdown> {
    let platform = kind.fee();
    Ok(Breakdown {
        platform_atoms: platform,
        network_atoms: network,
        asset_output_atoms: asset_output,
        total_atoms: platform
            .checked_add(network)
            .and_then(|n| n.checked_add(asset_output))
            .context("payment total overflow")?,
        platform_recipient: FEE_ADDRESS.into(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_fees() {
        for (kind, amount) in [
            (MintKind::Nft, 500_000_000_000),
            (MintKind::TokenDeploy, 1_000_000_000_000),
            (MintKind::Map, 750_000_000_000),
        ] {
            let output = fee_output(kind).unwrap();
            assert_eq!(output.value, amount);
            verify_fee(std::slice::from_ref(&output), kind).unwrap();
            let mut bad = output.clone();
            bad.value -= 1;
            assert!(verify_fee(&[bad], kind).is_err());
            let mut bad = output.clone();
            bad.script[2] ^= 1;
            assert!(verify_fee(&[bad], kind).is_err());
            assert!(verify_fee(&[output.clone(), output], kind).is_err());
            let b = breakdown(kind, 123, 456).unwrap();
            assert_eq!(b.total_atoms, amount + 579);
            assert_eq!(b.network_atoms, 123);
        }
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    #[test]
    fn settlement_commissions_and_duplicate_fee() {
        let fee = dex_commission(10_000 * ATOMS_PER_SOQ).unwrap();
        assert_eq!(fee, 50 * ATOMS_PER_SOQ);
        assert_eq!(dex_commission(201).unwrap(), 2);
        let carrier = Output {
            value: 0,
            script: vec![0x6a, 1, 0],
        };
        let payment = transfer_output().unwrap();
        verify_transfer_fee(&[carrier.clone(), payment.clone()]).unwrap();
        assert!(verify_transfer_fee(&[carrier, payment.clone(), payment]).is_err());
    }
}
