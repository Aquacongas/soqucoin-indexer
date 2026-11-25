//! Versioned fingerprint compositor.
use anyhow::Result;
use serde::Serialize;
use soq_core::sha256;

pub const VERSION: &str = "soq-fingerprint-v3";
pub const SIZE: usize = 512;

mod flow;

#[derive(Debug, Serialize)]
pub struct Parameters {
    pub renderer: &'static str,
    pub creator_script: String,
    pub txid: String,
    pub vout: u32,
    pub block_hash: String,
    pub height: u64,
    pub pattern: Pattern,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pattern {
    Loop,
    Whorl,
    Arch,
}

fn hash32(hex: &str) -> Result<[u8; 32]> {
    let mut hash = [0; 32];
    hex::decode_to_slice(hex, &mut hash)?;
    Ok(hash)
}

pub fn canonical(
    creator_script: &[u8; 34],
    txid: &str,
    vout: u32,
    block_hash: &str,
    height: u64,
) -> Result<(Parameters, Vec<u8>)> {
    let txid = hash32(txid)?;
    let block_hash = hash32(block_hash)?;
    let seed = sha256(
        &[
            b"soq-fingerprint-v1/canonical\0".as_slice(),
            creator_script,
            &txid,
            &vout.to_le_bytes(),
            &block_hash,
        ]
        .concat(),
    );
    let (pattern, png) = render(seed)?;
    Ok((
        Parameters {
            renderer: VERSION,
            creator_script: hex::encode(creator_script),
            txid: hex::encode(txid),
            vout,
            block_hash: hex::encode(block_hash),
            height,
            pattern,
        },
        png,
    ))
}

pub fn preview(creator_script: Option<&[u8]>, nonce: &[u8; 32]) -> Result<Vec<u8>> {
    let seed = sha256(
        &[
            b"soq-fingerprint-v1/preview\0".as_slice(),
            nonce,
            creator_script.unwrap_or_default(),
        ]
        .concat(),
    );
    Ok(render(seed)?.1)
}

fn render(seed: [u8; 32]) -> Result<(Pattern, Vec<u8>)> {
    let pattern = match seed[0] % 5 {
        0 | 1 => Pattern::Loop,
        2 | 3 => Pattern::Whorl,
        _ => Pattern::Arch,
    };
    let rgb = flow::compose(seed, pattern);
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, SIZE as u32, SIZE as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&rgb)?;
        writer.finish()?;
    }
    Ok((pattern, png))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_png_golden_vector() {
        let mut script = [1; 34];
        script[..2].copy_from_slice(&[0x51, 0x20]);
        let txid = "ab".repeat(32);
        let block = "cd".repeat(32);
        let (_, png) = canonical(&script, &txid, 3, &block, 20).unwrap();
        assert_eq!(
            hex::encode(sha256(&png)),
            "6820f25b5f358f08284e8fbe408a1fdad6208344933d82cdc4c6490c3e90b67e"
        );

        let (_, other_output) = canonical(&script, &txid, 4, &block, 20).unwrap();
        script[2] = 2;
        let (_, other_creator) = canonical(&script, &txid, 3, &block, 20).unwrap();
        assert_ne!(png, other_output);
        assert_ne!(png, other_creator);
    }
}
