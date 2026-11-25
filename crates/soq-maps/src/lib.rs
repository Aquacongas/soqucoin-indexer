//! Original SOQ renderer. Type/color/geometry use independent SHA-256 domains.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use soq_core::{hash_bytes, sha256};
pub const VERSION: &str = "soq-maps-1";
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Square,
    Bubble,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Palette {
    Black,
    Copper,
    Blue,
    Green,
    Purple,
}
impl Palette {
    pub fn foreground(self) -> &'static str {
        match self {
            Self::Black => "#000000",
            Self::Copper => "#D96F32",
            Self::Blue => "#2563EB",
            Self::Green => "#10B981",
            Self::Purple => "#9333EA",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Parameters {
    pub block_hash: String,
    pub height: u64,
    pub shape: Shape,
    pub palette: Palette,
    pub renderer: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Element {
    pub x: i32,
    pub y: i32,
    pub size: i32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artwork {
    pub name: String,
    pub parameters: Parameters,
    pub elements: Vec<Element>,
    pub svg: String,
    pub content_sha256: String,
}
struct Stream {
    seed: Vec<u8>,
    counter: u64,
}
impl Stream {
    fn new(hash: [u8; 32], domain: &[u8]) -> Self {
        let mut seed = b"SOQ.MAPS\0v1\0".to_vec();
        seed.extend(domain);
        seed.push(0);
        seed.extend(hash);
        Self { seed, counter: 0 }
    }
    fn below(&mut self, bound: u32) -> u32 {
        assert!(bound > 0);
        let limit = (1u64 << 32) - ((1u64 << 32) % bound as u64);
        loop {
            let mut b = self.seed.clone();
            b.extend(self.counter.to_le_bytes());
            self.counter += 1;
            let n = u32::from_le_bytes(sha256(&b)[..4].try_into().expect("hash prefix"));
            if (n as u64) < limit {
                return n % bound;
            }
        }
    }
}
fn shape_ticket(ticket: u32) -> Shape {
    if ticket == 0 {
        Shape::Bubble
    } else {
        Shape::Square
    }
}
fn color_ticket(ticket: u32) -> Palette {
    match ticket {
        0 => Palette::Copper,
        1 => Palette::Blue,
        2 => Palette::Green,
        3 => Palette::Purple,
        _ => Palette::Black,
    }
}
pub fn parameters(block_hash: String, height: u64) -> Result<Parameters> {
    let hash = hash_bytes(&block_hash)?;
    ensure!(hex::encode(hash) == block_hash, "hash must be lowercase");
    let shape = shape_ticket(Stream::new(hash, b"type").below(10000));
    let palette = color_ticket(Stream::new(hash, b"color").below(100));
    Ok(Parameters {
        block_hash,
        height,
        shape,
        palette,
        renderer: VERSION.into(),
    })
}
pub fn metadata(art: &Artwork) -> serde_json::Value {
    serde_json::json!({"name":art.name,"type":"soq-map","parameters":art.parameters,"content_sha256":art.content_sha256,"geometry_count":art.elements.len()})
}
pub fn metadata_hash(art: &Artwork) -> Result<[u8; 32]> {
    Ok(sha256(&serde_json::to_vec(&metadata(art))?))
}
pub fn overlaps(a: &Element, b: &Element, shape: Shape) -> bool {
    match shape {
        Shape::Square => {
            a.x < b.x + b.size + 4
                && b.x < a.x + a.size + 4
                && a.y < b.y + b.size + 4
                && b.y < a.y + a.size + 4
        }
        Shape::Bubble => {
            let dx = a.x - b.x;
            let dy = a.y - b.y;
            let distance = a.size + b.size + 4;
            dx * dx + dy * dy < distance * distance
        }
    }
}
pub fn generate(p: Parameters) -> Result<Artwork> {
    ensure!(
        p == parameters(p.block_hash.clone(), p.height)?,
        "type, color and version must derive from canonical block hash"
    );
    let hash = hash_bytes(&p.block_hash)?;
    let mut rng = Stream::new(hash, b"geometry");
    let target = 8 + rng.below(57);
    let mut elements = Vec::new();
    for _ in 0..4096 {
        if elements.len() == target as usize {
            break;
        }
        let size = 8 + rng.below(65) as i32;
        let e = match p.shape {
            Shape::Square => Element {
                x: 16 + rng.below((480 - size) as u32) as i32,
                y: 16 + rng.below((480 - size) as u32) as i32,
                size,
            },
            Shape::Bubble => Element {
                x: 16 + size + rng.below((480 - 2 * size) as u32) as i32,
                y: 16 + size + rng.below((480 - 2 * size) as u32) as i32,
                size,
            },
        };
        if elements.iter().all(|b| !overlaps(&e, b, p.shape)) {
            elements.push(e);
        }
    }
    let fg = p.palette.foreground();
    let mut svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 512 512\"><rect width=\"512\" height=\"512\" fill=\"#FFFFFF\"/><g fill=\"{fg}\">");
    for e in &elements {
        svg.push_str(&match p.shape {
            Shape::Square => format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                e.x, e.y, e.size, e.size
            ),
            Shape::Bubble => format!("<circle cx=\"{}\" cy=\"{}\" r=\"{}\"/>", e.x, e.y, e.size),
        });
    }
    svg.push_str("</g></svg>");
    let content_sha256 = hex::encode(sha256(svg.as_bytes()));
    let suffix = match p.shape {
        Shape::Square => "soqmap",
        Shape::Bubble => "soqububblemap",
    };
    let name = format!("{}.{suffix}", p.height);
    Ok(Artwork {
        name,
        parameters: p,
        elements,
        svg,
        content_sha256,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_probability_partition() {
        assert_eq!(
            (0..10000)
                .filter(|n| shape_ticket(*n) == Shape::Bubble)
                .count(),
            1
        );
        for palette in [
            Palette::Copper,
            Palette::Blue,
            Palette::Green,
            Palette::Purple,
        ] {
            assert_eq!((0..100).filter(|n| color_ticket(*n) == palette).count(), 1);
        }
        assert_eq!(
            (0..100)
                .filter(|n| color_ticket(*n) == Palette::Black)
                .count(),
            96
        );
        let hash = [42; 32];
        assert_ne!(
            Stream::new(hash, b"type").seed,
            Stream::new(hash, b"color").seed
        );
        for bound in [100u64, 10000] {
            let limit = (1u64 << 32) - ((1u64 << 32) % bound);
            assert_eq!(limit % bound, 0);
            assert!(limit <= 1u64 << 32);
        }
    }
    #[test]
    fn deterministic_geometry() {
        let mut bubble_seen = false;
        for n in 0..20000u32 {
            let hash = hex::encode(sha256(&n.to_le_bytes()));
            let p = parameters(hash, n as u64).unwrap();
            if n > 128 && p.shape != Shape::Bubble {
                continue;
            }
            let a = generate(p.clone()).unwrap();
            assert_eq!(a.svg, generate(p.clone()).unwrap().svg);
            assert!(!a.elements.is_empty());
            for (i, e) in a.elements.iter().enumerate() {
                assert!(e.size > 0);
                for b in &a.elements[..i] {
                    assert!(!overlaps(e, b, p.shape));
                }
                match p.shape {
                    Shape::Square => {
                        assert!(e.x >= 0 && e.y >= 0 && e.x + e.size <= 512 && e.y + e.size <= 512)
                    }
                    Shape::Bubble => assert!(
                        e.x - e.size >= 0
                            && e.y - e.size >= 0
                            && e.x + e.size <= 512
                            && e.y + e.size <= 512
                    ),
                }
            }
            assert_eq!(
                a.name,
                format!(
                    "{}.{}",
                    p.height,
                    if p.shape == Shape::Square {
                        "soqmap"
                    } else {
                        "soqububblemap"
                    }
                )
            );
            bubble_seen |= p.shape == Shape::Bubble;
        }
        assert!(bubble_seen);
    }
    #[test]
    fn reject_manual_shape_and_color() {
        let mut p = parameters("00".repeat(32), 0).unwrap();
        p.shape = if p.shape == Shape::Square {
            Shape::Bubble
        } else {
            Shape::Square
        };
        assert!(generate(p).is_err());
    }
}

pub mod synthwave;
