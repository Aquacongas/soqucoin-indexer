//! SOQ.MAPS renderer. Geometry follows the NOID.MAPS/TAPE.MAPS weighted quadtree.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use soq_core::{hash_bytes, sha256};

const SIZE: usize = 576;
const VERSION: &str = "soq-maps-quadtree-2";
const PALETTE: [[u8; 3]; 5] = [
    [0x00, 0xBF, 0xFF], // Electric Blue
    [0xFF, 0x00, 0xA8], // Neon Pink
    [0xA8, 0x55, 0xFF], // Neon Purple
    [0x39, 0xFF, 0x14], // Neon Green
    [0xFF, 0x67, 0x00], // Neon Orange
];
const MAP_TYPE_DOMAIN: u8 = 1;
const GEOMETRY_DOMAIN: u8 = 2;
const RARITY_DOMAIN: u8 = 3;
const COLOR_DOMAIN: u8 = 4;
const COLORED_COUNT_DOMAIN: u8 = 5;
const ELEMENT_SELECTION_DOMAIN: u8 = 6;
type Square = (u64, u64, u64);

struct Stream {
    domain_seed: [u8; 32],
    counter: u64,
}

impl Stream {
    fn new(seed: [u8; 32], domain: u8, index: u64) -> Self {
        let mut bytes = [0; 41];
        bytes[..32].copy_from_slice(&seed);
        bytes[32] = domain;
        bytes[33..].copy_from_slice(&index.to_be_bytes());
        Self {
            domain_seed: sha256(&bytes),
            counter: 0,
        }
    }

    fn below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0);
        let threshold = bound.wrapping_neg() % bound;
        let mut bytes = [0; 40];
        bytes[..32].copy_from_slice(&self.domain_seed);
        loop {
            bytes[32..].copy_from_slice(&self.counter.to_be_bytes());
            self.counter += 1;
            let ticket = u64::from_be_bytes(sha256(&bytes)[..8].try_into().unwrap());
            if ticket >= threshold {
                return ticket % bound;
            }
        }
    }
}

fn seed(block_hash: &str, txid: &str, creator: &[u8; 34]) -> Result<[u8; 32]> {
    ensure!(
        creator[..2] == [0x51, 32],
        "invalid creator witness address"
    );
    // Fixed widths: displayed hash bytes, displayed txid bytes, binary witness v1 address.
    let mut bytes = b"SOQ.MAPS\0seed\0v1\0".to_vec();
    bytes.extend(hash_bytes(block_hash)?);
    bytes.extend(hash_bytes(txid)?);
    bytes.extend(creator);
    Ok(sha256(&bytes))
}

fn squares(mut rng: Stream) -> Vec<Square> {
    let scale = 1 << rng.below(10);
    let count = scale + rng.below(scale);
    let mut squares = if count == 1 {
        vec![(0, 0, 24 + rng.below(521))]
    } else {
        let extent = 400 + rng.below(145);
        let mut cells: Vec<Square> = vec![(0, 0, extent)];
        let capacity = count + rng.below(1 + count / 3);
        while (cells.len() as u64) < capacity {
            let weights: Vec<_> = cells
                .iter()
                .map(|&(_, _, side)| if side >= 12 { side.pow(3) } else { 0 })
                .collect();
            let mut draw = rng.below(weights.iter().sum());
            let index = weights
                .iter()
                .position(|&weight| {
                    if draw < weight {
                        return true;
                    }
                    draw -= weight;
                    false
                })
                .expect("weighted cell");
            let (x, y, side) = cells.remove(index);
            let half = side / 2;
            let far = side - half;
            cells.extend([
                (x, y, half),
                (x + far, y, half),
                (x, y + far, half),
                (x + far, y + far, half),
            ]);
        }
        for i in (1..cells.len()).rev() {
            cells.swap(i, rng.below((i + 1) as u64) as usize);
        }
        cells
            .into_iter()
            .take(count as usize)
            .map(|(x, y, cell)| {
                let low = 3.max(cell * 2 / 3);
                let side = low + rng.below(cell - 1 - low);
                (
                    x + 1 + rng.below(cell - side - 1),
                    y + 1 + rng.below(cell - side - 1),
                    side,
                )
            })
            .collect()
    };
    let left = squares.iter().map(|s| s.0).min().unwrap();
    let top = squares.iter().map(|s| s.1).min().unwrap();
    let right = squares.iter().map(|s| s.0 + s.2).max().unwrap();
    let bottom = squares.iter().map(|s| s.1 + s.2).max().unwrap();
    for (x, y, _) in &mut squares {
        *x = *x - left + (SIZE as u64 - (right - left)) / 2;
        *y = *y - top + (SIZE as u64 - (bottom - top)) / 2;
    }
    squares
}

fn colors(seed: [u8; 32], count: usize) -> Vec<[u8; 3]> {
    let mut rarity = Stream::new(seed, RARITY_DOMAIN, 0);
    let rainbow = rarity.below(1_000) == 0 && count >= PALETTE.len();
    let colored = !rainbow && rarity.below(100) == 0;
    let mut result = vec![PALETTE[0]; count];
    if !rainbow && !colored {
        return result;
    }

    let mut color = Stream::new(seed, COLOR_DOMAIN, 0);
    let mut selection = Stream::new(seed, ELEMENT_SELECTION_DOMAIN, 0);
    let mut elements: Vec<_> = (0..count).collect();
    if rainbow {
        // Sample a distinct position for every palette entry before filling
        // the remaining positions from the entire palette.
        for (i, foreground) in PALETTE.iter().enumerate() {
            let j = i + selection.below((count - i) as u64) as usize;
            elements.swap(i, j);
            result[elements[i]] = *foreground;
        }
        for &index in &elements[PALETTE.len()..] {
            result[index] = PALETTE[color.below(PALETTE.len() as u64) as usize];
        }
    } else {
        let accent = PALETTE[1 + color.below((PALETTE.len() - 1) as u64) as usize];
        let colored_count = 1 + Stream::new(seed, COLORED_COUNT_DOMAIN, 0).below(count as u64);
        for i in 0..colored_count as usize {
            let j = i + selection.below((count - i) as u64) as usize;
            elements.swap(i, j);
            result[elements[i]] = accent;
        }
    }
    result
}

pub fn render(
    block_hash: &str,
    height: u64,
    txid: &str,
    creator: &[u8; 34],
) -> Result<(Value, Vec<u8>)> {
    let seed = seed(block_hash, txid, creator)?;
    let bubble = Stream::new(seed, MAP_TYPE_DOMAIN, 0).below(10_000) == 0;
    let squares = squares(Stream::new(seed, GEOMETRY_DOMAIN, 0));
    let colors = colors(seed, squares.len());
    let mut pixels = vec![0; SIZE * SIZE * 3];
    for (&(x, y, side), foreground) in squares.iter().zip(colors) {
        for dy in 0..side {
            for dx in 0..side {
                if bubble {
                    let cx = (2 * dx + 1).abs_diff(side);
                    let cy = (2 * dy + 1).abs_diff(side);
                    if cx * cx + cy * cy > side * side {
                        continue;
                    }
                }
                let offset = (((y + dy) as usize * SIZE) + (x + dx) as usize) * 3;
                pixels[offset..offset + 3].copy_from_slice(&foreground);
            }
        }
    }
    let mut image = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut image, SIZE as u32, SIZE as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_compression(png::Compression::Fast);
        encoder.write_header()?.write_image_data(&pixels)?;
    }
    let art = json!({
        "name": format!("{height}.soqumap"),
        "type": "soq-map",
        "parameters": {
            "renderer": VERSION, "height": height, "block_hash": block_hash,
            "shape": if bubble { "bubble" } else { "square" },
            "txid": txid, "creator_script": hex::encode(creator),
            "geometry_count": squares.len()
        },
        "content_sha256": hex::encode(sha256(&image))
    });
    Ok((art, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_geometry() {
        let mut dense = false;
        for n in 0..30u8 {
            let elements = squares(Stream::new([n; 32], 2, 0));
            dense |= elements.len() > 64;
            for (i, &(x, y, side)) in elements.iter().enumerate() {
                assert!(side > 0 && x + side <= SIZE as u64 && y + side <= SIZE as u64);
                for &(a, b, size) in &elements[..i] {
                    assert!(x + side <= a || a + size <= x || y + side <= b || b + size <= y);
                }
            }
        }
        assert!(dense);
    }

    #[test]
    fn map_level_rarity_and_color_assignment() {
        let normal = sha256(&0u32.to_be_bytes());
        let color = sha256(&112u32.to_be_bytes());
        // This seed hits both rarity draws. Rainbow takes precedence when
        // eligible, and the Color draw still applies below five elements.
        let rainbow = sha256(&55_514u32.to_be_bytes());
        assert_eq!(colors(normal, 23), vec![PALETTE[0]; 23]);

        for count in [1, 4, 29] {
            let result = colors(color, count);
            let accent = PALETTE[1 + Stream::new(color, COLOR_DOMAIN, 0).below(4) as usize];
            let k = 1 + Stream::new(color, COLORED_COUNT_DOMAIN, 0).below(count as u64);
            assert_eq!(result.len(), count);
            assert_eq!(
                result.iter().filter(|&&entry| entry == accent).count(),
                k as usize
            );
            assert!(result
                .iter()
                .all(|&entry| entry == accent || entry == PALETTE[0]));
        }

        for count in [5, 23] {
            let result = colors(rainbow, count);
            assert_eq!(result.len(), count);
            assert!(PALETTE.iter().all(|entry| result.contains(entry)));
            assert_eq!(result, colors(rainbow, count));
        }
        let small = colors(rainbow, 4);
        let accent = PALETTE[1 + Stream::new(rainbow, COLOR_DOMAIN, 0).below(4) as usize];
        assert_eq!(small.len(), 4);
        assert!(small.contains(&accent));
        assert!(small
            .iter()
            .all(|&entry| entry == accent || entry == PALETTE[0]));
    }

    #[test]
    fn canonical_inputs_and_bubble_png() {
        let txid = "12".repeat(32);
        let mut creator = [7; 34];
        creator[..2].copy_from_slice(&[0x51, 32]);
        let block = (0..100_000u64)
            .map(|n| format!("{n:064x}"))
            .find(|hash| Stream::new(seed(hash, &txid, &creator).unwrap(), 1, 0).below(10_000) == 0)
            .unwrap();
        let (art, png) = render(&block, 123, &txid, &creator).unwrap();
        assert_eq!(art["name"], "123.soqumap");
        assert_eq!(art["parameters"]["shape"], "bubble");
        let canonical_seed = seed(&block, &txid, &creator).unwrap();
        let geometry = squares(Stream::new(canonical_seed, GEOMETRY_DOMAIN, 0));
        let count = geometry.len();
        assert_eq!(art["parameters"]["geometry_count"], count);
        let mut reader = png::Decoder::new(std::io::Cursor::new(&png))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!(&pixels[..3], &[0, 0, 0]);
        for ((x, y, side), expected) in geometry.into_iter().zip(colors(canonical_seed, count)) {
            let center = ((y + side / 2) as usize * SIZE + (x + side / 2) as usize) * 3;
            assert_eq!(pixels[center..center + 3], expected);
        }
        assert_eq!(art["content_sha256"], hex::encode(sha256(&png)));
        assert_eq!((art, png), render(&block, 123, &txid, &creator).unwrap());
        let original = seed(&block, &txid, &creator).unwrap();
        assert_ne!(original, seed(&txid, &txid, &creator).unwrap());
        assert_ne!(original, seed(&block, &block, &creator).unwrap());
        creator[33] ^= 1;
        assert_ne!(original, seed(&block, &txid, &creator).unwrap());
    }
}
