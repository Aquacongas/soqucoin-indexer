use std::collections::VecDeque;
use std::ops::{Add, Mul, Sub};

use super::{Pattern, SIZE};

const STEP: f64 = 1.8;
const CELL: usize = 12;
const GRID: usize = SIZE.div_ceil(CELL);
const CENTER: Point = Point::new(256.0, 256.0);

#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    fn normal(self) -> Self {
        Self::new(-self.y, self.x)
    }

    fn unit(self) -> Self {
        self * (1.0 / self.length().max(1e-9))
    }

    fn complex_mul(self, other: Self) -> Self {
        Self::new(
            self.x * other.x - self.y * other.y,
            self.x * other.y + self.y * other.x,
        )
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Point {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

struct Field {
    axes: Point,
    rotation: Point,
    shift: Point,
    cores: Vec<Point>,
    deltas: Vec<Point>,
    handedness: f64,
    crest: f64,
    bend: f64,
    arch_rise: f64,
    boundary: [f64; 4],
    phase: [f64; 3],
    period: f64,
    width: f64,
    junction_rate: u8,
}

impl Field {
    fn new(seed: [u8; 32], pattern: Pattern) -> Self {
        let sample = |i: usize| f64::from(seed[i]) / 255.0;
        let signed = |i| sample(i) * 2.0 - 1.0;
        let core = Point::new(signed(3) * 47.0, -49.0 + signed(4) * 33.0);
        let (cores, deltas) = match pattern {
            Pattern::Arch => (vec![], vec![]),
            Pattern::Loop => (
                vec![core],
                vec![core + Point::new(78.0 + signed(13) * 27.0, 98.0 + signed(14) * 28.0)],
            ),
            Pattern::Whorl => (
                vec![
                    core,
                    core + Point::new(signed(13) * 37.0, 54.0 + signed(14) * 22.0),
                ],
                vec![
                    Point::new(-113.0 + signed(15) * 29.0, 62.0 + signed(16) * 37.0),
                    Point::new(114.0 + signed(17) * 30.0, 75.0 + signed(18) * 39.0),
                ],
            ),
        };
        let (sin, cos) = libm::sincos(signed(5) * 0.32);
        Self {
            axes: Point::new(163.0 + signed(7) * 22.0, 204.0 + signed(8) * 18.0),
            rotation: Point::new(cos, sin),
            shift: Point::new(signed(19) * 13.0, signed(20) * 12.0),
            cores,
            deltas,
            handedness: if seed[6] & 1 == 0 { 1.0 } else { -1.0 },
            crest: signed(9) * 58.0,
            bend: 0.35 + sample(10) * 0.43,
            arch_rise: match pattern {
                Pattern::Arch => 0.38 + sample(27) * 0.39,
                _ => 0.0,
            },
            boundary: [signed(11), signed(12), signed(21), signed(22)],
            phase: [sample(23) * 6.28, sample(24) * 6.28, sample(25) * 6.28],
            period: 8.8 + sample(1) * 2.5,
            width: 1.35 + sample(2) * 0.52,
            junction_rate: 28 + seed[26] % 40,
        }
    }

    fn local(&self, p: Point) -> Point {
        let p = p - CENTER - self.shift;
        let p = p.complex_mul(Point::new(self.rotation.x, -self.rotation.y));
        Point::new(p.x * self.handedness, p.y)
    }

    fn boundary_margin(&self, p: Point) -> f64 {
        let p = self.local(p);
        let y = p.y / self.axes.y;
        let x = (p.x - 14.0 * y * y - self.boundary[0] * 19.0 * y)
            / (self.axes.x * (1.0 - (0.12 + self.boundary[1] * 0.10) * y));
        let radius = Point::new(x, y).length();
        let u = x / radius.max(0.01);
        let v = y / radius.max(0.01);
        let edge = 1.0
            + 0.085 * self.boundary[2] * u * (1.0 - 4.0 * v * v)
            + 0.075 * self.boundary[3] * (1.0 - 8.0 * u * u * v * v)
            + 0.032 * (u * u - v * v) * (2.0 * u * v);
        edge - radius
    }

    fn contains(&self, p: Point) -> bool {
        self.boundary_margin(p) > 0.0
    }

    fn singular(&self, p: Point, distance: f64) -> bool {
        let p = self.local(p);
        self.cores
            .iter()
            .chain(&self.deltas)
            .any(|&s| (p - s).length() < distance)
    }

    fn direction(&self, p: Point) -> Point {
        let p = self.local(p);
        let x = (p.x - self.crest) / 115.0;
        let arch_height = (p.y + 29.0) / 104.0;
        let arch = self.arch_rise / (1.0 + arch_height * arch_height);
        let background = (self.bend + arch) * x / (1.0 + 0.55 * x * x)
            + 0.10 * libm::sin(p.y * 0.017 + p.x * 0.011 + self.phase[0])
            + 0.055 * libm::sin(p.x * 0.039 - p.y * 0.023 + self.phase[1]);
        let (sin, cos) = libm::sincos(2.0 * background);
        let mut doubled = Point::new(cos, sin);
        // The argument of the zero-pole field is twice the ridge angle.
        // Cores turn the line field by +π and deltas turn it by −π.
        for &core in &self.cores {
            doubled = doubled.complex_mul((p - core).unit());
        }
        for &delta in &self.deltas {
            let offset = (p - delta).unit();
            doubled = doubled.complex_mul(Point::new(offset.x, -offset.y));
        }
        let x = (0.5 * (1.0 + doubled.x)).max(0.0).sqrt();
        let y = (0.5 * (1.0 - doubled.x))
            .max(0.0)
            .sqrt()
            .copysign(doubled.y);
        Point::new(x * self.handedness, y).complex_mul(self.rotation)
    }

    fn spacing(&self, p: Point) -> f64 {
        self.period
            + 1.15 * libm::sin(p.x * 0.021 - p.y * 0.013 + self.phase[1])
            + 0.55 * libm::sin(p.y * 0.041 + self.phase[2])
    }

    fn thickness(&self, p: Point, offset: f64) -> f64 {
        self.width
            + offset
            + 0.25 * libm::sin(p.x * 0.043 + p.y * 0.032 + self.phase[0])
            + 0.16 * libm::sin(p.x * 0.083 - p.y * 0.068 + self.phase[2])
    }
}

struct Ridges {
    cells: Vec<Vec<Point>>,
}

impl Ridges {
    fn new() -> Self {
        Self {
            cells: vec![Vec::new(); GRID * GRID],
        }
    }

    fn cell(p: Point) -> (usize, usize) {
        (
            (p.x as usize / CELL).min(GRID - 1),
            (p.y as usize / CELL).min(GRID - 1),
        )
    }

    fn nearest(&self, p: Point, radius: f64) -> Option<Point> {
        debug_assert!(radius <= CELL as f64);
        let (x, y) = Self::cell(p);
        let mut best = None;
        let mut distance = radius * radius;
        for row in y.saturating_sub(1)..=(y + 1).min(GRID - 1) {
            for col in x.saturating_sub(1)..=(x + 1).min(GRID - 1) {
                for &q in &self.cells[row * GRID + col] {
                    let delta = q - p;
                    let squared = delta.dot(delta);
                    if squared < distance {
                        best = Some(q);
                        distance = squared;
                    }
                }
            }
        }
        best
    }

    fn insert(&mut self, line: &[Point]) {
        for &p in line {
            let (x, y) = Self::cell(p);
            self.cells[y * GRID + x].push(p);
        }
    }
}

fn oriented(field: &Field, p: Point, reference: Point) -> Point {
    let tangent = field.direction(p);
    tangent * 1.0_f64.copysign(tangent.dot(reference))
}

fn trace(field: &Field, ridges: &Ridges, start: Point, mut tangent: Point) -> (Vec<Point>, bool) {
    let initial_direction = tangent;
    let mut line = vec![start];
    let mut p = start;
    for _ in 0..900 {
        let middle = p + tangent * (STEP * 0.5);
        let direction = oriented(field, middle, tangent);
        let next = p + direction * STEP;
        if !field.contains(next)
            || field.singular(next, 2.8)
            || ridges.nearest(next, field.spacing(next) * 0.68).is_some()
        {
            break;
        }
        if line.len() > 12
            && (next - start).length() < STEP * 1.25
            && direction.dot(initial_direction) > 0.8
        {
            line.push(start);
            return (line, true);
        }
        // Stop a tight spiral from doubling back over its own earlier turns.
        if line
            .iter()
            .rev()
            .skip(8)
            .step_by(3)
            .any(|&q| (next - q).length() < 4.0)
        {
            break;
        }
        line.push(next);
        p = next;
        tangent = oriented(field, p, direction);
    }
    (line, false)
}

fn join(field: &Field, ridges: &Ridges, line: &mut Vec<Point>) -> bool {
    let last = line.len() - 1;
    let tip = line[last];
    if last < 10 || field.boundary_margin(tip) < 0.09 {
        return false;
    }
    let trim = last - 4;
    let end = line[trim];
    let incoming = (end - line[trim - 2]).unit();
    let Some(target) = ridges.nearest(tip + incoming * 10.0, 9.0) else {
        return false;
    };
    let chord = target - end;
    let length = chord.length();
    let outgoing = oriented(field, target, incoming);
    if !(12.0..=25.0).contains(&length)
        || chord.unit().dot(incoming) < 0.82
        || chord.unit().dot(outgoing) < 0.75
        || field.singular(target, 6.0)
    {
        return false;
    }
    // A cubic joins the two tangents, avoiding a hard corner at the fork.
    let a = end + incoming * (length * 0.40);
    let b = target - outgoing * (length * 0.33);
    let steps = (length / STEP).ceil() as usize + 2;
    let mut connector = Vec::with_capacity(steps);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let s = 1.0 - t;
        let p = end * (s * s * s)
            + a * (3.0 * s * s * t)
            + b * (3.0 * s * t * t)
            + target * (t * t * t);
        if !field.contains(p) || ((p - target).length() > 5.0 && ridges.nearest(p, 3.5).is_some()) {
            return false;
        }
        connector.push(p);
    }
    line.truncate(trim + 1);
    line.extend(connector);
    true
}

fn stroke(ink: &mut [u8], a: Point, b: Point, wa: f64, wb: f64) {
    let margin = wa.max(wb) + 0.8;
    let left = (a.x.min(b.x) - margin).floor().max(0.0) as usize;
    let top = (a.y.min(b.y) - margin).floor().max(0.0) as usize;
    let right = ((a.x.max(b.x) + margin).ceil() as usize).min(SIZE - 1);
    let bottom = ((a.y.max(b.y) + margin).ceil() as usize).min(SIZE - 1);
    let segment = b - a;
    let inverse_length = 1.0 / segment.dot(segment).max(1e-9);
    for y in top..=bottom {
        for x in left..=right {
            let offset = Point::new(x as f64 + 0.5, y as f64 + 0.5) - a;
            let t = (offset.dot(segment) * inverse_length).clamp(0.0, 1.0);
            let distance = (offset - segment * t).length();
            let alpha = (wa + (wb - wa) * t - distance + 0.5).clamp(0.0, 1.0);
            let pixel = &mut ink[y * SIZE + x];
            *pixel = (*pixel).max((alpha * 255.0 + 0.5) as u8);
        }
    }
}

pub(super) fn compose(seed: [u8; 32], pattern: Pattern) -> Vec<u8> {
    let field = Field::new(seed, pattern);
    let mut ridges = Ridges::new();
    let mut ink = vec![0; SIZE * SIZE];
    let mut random = u64::from_le_bytes(seed[24..].try_into().unwrap());
    let mut draw = || {
        // SplitMix64 keeps the candidate order local to this render.
        random = random.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = random;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) >> 56) as u8
    };

    let mut candidates = Vec::new();
    for y in (5..SIZE).step_by(6) {
        for x in (5..SIZE).step_by(6) {
            let p = Point::new(
                x as f64 + f64::from(draw()) / 51.0,
                y as f64 + f64::from(draw()) / 51.0,
            );
            if field.contains(p) {
                candidates.push(p);
            }
        }
    }
    // Fill the core first, then propagate seeds along the normals of each
    // accepted ridge. Jittered candidates fill pockets left by that process.
    candidates.sort_by_key(|&p| {
        let local = field.local(p);
        let anchor = field
            .cores
            .first()
            .copied()
            .unwrap_or(Point::new(field.crest, -40.0));
        ((local - anchor).length() * 0.3) as u32 + u32::from(seed[((p.x + p.y) as usize) % 32] / 10)
    });
    let mut neighbors = VecDeque::new();
    let mut fallback = candidates.into_iter();
    while let Some(p) = neighbors.pop_front().or_else(|| fallback.next()) {
        let spacing = field.spacing(p);
        if !field.contains(p)
            || field.singular(p, 4.0)
            || ridges.nearest(p, spacing * 0.84).is_some()
        {
            continue;
        }

        let direction = field.direction(p);
        let (mut line, mut closed) = trace(&field, &ridges, p, direction);
        if !closed {
            let (mut reverse, reverse_closed) = trace(&field, &ridges, p, direction * -1.0);
            if !reverse_closed {
                reverse.reverse();
                reverse.pop();
                reverse.append(&mut line);
            }
            line = reverse;
            closed = reverse_closed;
        }
        if line.len() < 6 {
            continue;
        }

        for pair in line.windows(2).step_by(6) {
            let p = pair[0];
            let normal = (pair[1] - p).unit().normal();
            let distance = field.spacing(p) * (0.97 + f64::from(draw()) / 2400.0);
            neighbors.push_back(p + normal * distance);
            neighbors.push_back(p - normal * distance);
        }
        let mut joined = [false; 2];
        if !closed {
            joined[1] = draw() < field.junction_rate && join(&field, &ridges, &mut line);
            if draw() < field.junction_rate {
                line.reverse();
                joined.reverse();
                joined[1] = join(&field, &ridges, &mut line);
            }
        }

        ridges.insert(&line);
        let offset = (f64::from(draw()) / 255.0 - 0.5) * 0.54;
        let last = line.len() - 1;
        for (i, pair) in line.windows(2).enumerate() {
            let width = |j: usize, p: Point| {
                let start = if joined[0] { 4 } else { j };
                let end = if joined[1] { 4 } else { last - j };
                let taper = if closed {
                    1.0
                } else {
                    0.54 + 0.46 * (start.min(end) as f64 / 4.0).min(1.0)
                };
                field.thickness(p, offset) * taper
            };
            stroke(
                &mut ink,
                pair[0],
                pair[1],
                width(i, pair[0]),
                width(i + 1, pair[1]),
            );
        }
    }
    ink.into_iter().flat_map(|alpha| [alpha, 0, 0]).collect()
}
