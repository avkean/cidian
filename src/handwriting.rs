use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

const SAMPLES: usize = 8;
const GRID: usize = 16;
const CELLS: usize = GRID * GRID;
const MAX_STROKES: usize = 48;
const MAX_POINTS: usize = 4096;
/// medians use a 1024-unit em square, baseline at 900
const FONT_UNITS: f32 = 1024.0;
const FONT_BASELINE: f32 = 900.0;
const SHORTLIST: usize = 512;
const FINALISTS: usize = 96;
const CANDIDATES: usize = 12;
const QUICK_COUNT_COST: f32 = 0.008;
const COUNT_COST: f32 = 0.001;
/// With few strokes, joining two distorts assignment more than ink shape.
const INK_WEIGHT: f32 = 0.30;
const FEW_STROKES_INK_WEIGHT: f32 = 0.50;
const FEW_STROKES: usize = 4;
const SCORE_FALLOFF: f32 = 5.0;
const THIN_AXIS: f32 = 0.2;
const MIN_EXTENT: f32 = 0.01;
const INK_STEPS_PER_CELL: f32 = 1.5;
const ORIENTATION_COST: [u8; 3] = [0, 2, 4];
const CENTER_WEIGHT: f32 = 0.6;
const UNMATCHED_COST: f32 = 0.05;
const UNMATCHED_LENGTH_COST: f32 = 0.25;
/// A template must not ignore drawn dots it cannot explain.
const UNEXPLAINED_INK_COST: f32 = 0.2;
const MISSING_STROKE_COST: f32 = 0.01;
const EXTRA_STROKE_COST: f32 = 0.01;
const RANK_STRENGTH: f32 = 0.45;
const MIN_WEIGHT: f32 = 0.001;
const EPSILON: f32 = 0.00001;
type Point = [f32; 2];
type Stroke = [Point; SAMPLES];

pub fn valid_strokes(strokes: &[Vec<Point>]) -> bool {
    !strokes.is_empty()
        && strokes.len() <= MAX_STROKES
        && strokes.iter().map(Vec::len).sum::<usize>() <= MAX_POINTS
        && strokes.iter().all(|stroke| {
            !stroke.is_empty()
                && stroke
                    .iter()
                    .flatten()
                    .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Character {
    pub character: String,
    pub pinyin: Vec<String>,
    pub definition: String,
    pub decomposition: String,
    pub radical: String,
    pub strokes: Vec<String>,
    pub medians: Vec<Vec<Point>>,
}

#[derive(Debug, Serialize)]
pub struct HandwritingCandidate {
    pub character: String,
    pub score: f32,
}

struct Ink {
    pixels: Vec<u16>,
    distances: [u8; CELLS * 4],
}

pub struct HandwritingIndex {
    characters: Vec<Character>,
    templates: Vec<Vec<Stroke>>,
    ink: Vec<Ink>,
    by_character: HashMap<char, usize>,
}

impl HandwritingIndex {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut characters = Vec::new();
        let mut templates = Vec::new();
        let mut ink = Vec::new();
        let mut by_character = HashMap::new();
        let mut rejected = 0;
        for (line_number, line) in BufReader::new(File::open(path)?).lines().enumerate() {
            let character: Character = serde_json::from_str(&line?).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("character line {}: {error}", line_number + 1),
                )
            })?;
            let mut chars = character.character.chars();
            let Some(key) = chars.next() else {
                rejected += 1;
                continue;
            };
            if chars.next().is_some()
                || character.medians.is_empty()
                || character.medians.len() > MAX_STROKES
            {
                rejected += 1;
                continue;
            }
            let medians = from_font(&character.medians);
            if medians.iter().any(|stroke| {
                stroke.is_empty() || stroke.iter().flatten().any(|value| !value.is_finite())
            }) {
                rejected += 1;
                continue;
            }
            by_character.insert(key, characters.len());
            let template = equalize(normalize(&medians));
            ink.push(ink_signature(&template));
            templates.push(template);
            characters.push(character);
        }
        eprintln!(
            "Handwriting: loaded {} characters, rejected {rejected} records",
            characters.len()
        );
        if characters.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Handwriting index has no usable characters",
            ));
        }
        Ok(Self {
            characters,
            templates,
            ink,
            by_character,
        })
    }

    pub fn lookup(&self, character: char) -> Option<&Character> {
        self.by_character
            .get(&character)
            .map(|&index| &self.characters[index])
    }

    pub fn recognize(&self, strokes: &[Vec<Point>]) -> Vec<HandwritingCandidate> {
        if !valid_strokes(strokes) {
            return Vec::new();
        }
        let drawn = equalize(normalize(strokes));
        let drawn_ink = ink_signature(&drawn);
        let count_gap = |index: usize| self.templates[index].len().abs_diff(drawn.len()) as f32;
        let by_score = |a: &(usize, f32), b: &(usize, f32)| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0));
        let mut shortlist: Vec<(usize, f32)> = self
            .ink
            .iter()
            .enumerate()
            .map(|(index, template)| {
                let quick = directed_cost(template, &drawn_ink);
                (index, quick + QUICK_COUNT_COST * count_gap(index))
            })
            .collect();
        if shortlist.len() > SHORTLIST {
            shortlist.select_nth_unstable_by(SHORTLIST, by_score);
            shortlist.truncate(SHORTLIST);
        }
        for (index, score) in &mut shortlist {
            *score = ink_cost(&drawn_ink, &self.ink[*index]) + COUNT_COST * count_gap(*index);
        }
        if shortlist.len() > FINALISTS {
            shortlist.select_nth_unstable_by(FINALISTS, by_score);
            shortlist.truncate(FINALISTS);
        }
        let few = FEW_STROKES.saturating_sub(drawn.len()) as f32 / (FEW_STROKES - 1) as f32;
        let ink_weight = INK_WEIGHT + FEW_STROKES_INK_WEIGHT * few;
        for (index, score) in &mut shortlist {
            let template = &self.templates[*index];
            let count = if drawn.len() < template.len() {
                MISSING_STROKE_COST * (template.len() - drawn.len()) as f32
            } else {
                EXTRA_STROKE_COST * drawn.len().saturating_sub(template.len() + 1) as f32
            };
            let alignment = alignment_cost(&drawn, template) + count;
            *score = ink_weight * ink_cost(&drawn_ink, &self.ink[*index])
                + (1.0 - ink_weight) * alignment;
        }
        shortlist.sort_unstable_by(|a, b| {
            a.1.total_cmp(&b.1).then_with(|| {
                self.characters[a.0]
                    .character
                    .cmp(&self.characters[b.0].character)
            })
        });
        shortlist
            .into_iter()
            .take(CANDIDATES)
            .map(|(index, distance)| HandwritingCandidate {
                character: self.characters[index].character.clone(),
                // for ordering only, not a probability
                score: (-SCORE_FALLOFF * distance).exp(),
            })
            .collect()
    }
}

fn from_font(medians: &[Vec<Point>]) -> Vec<Vec<Point>> {
    medians
        .iter()
        .map(|stroke| {
            stroke
                .iter()
                .map(|point| {
                    [
                        point[0] / FONT_UNITS,
                        (FONT_BASELINE - point[1]) / FONT_UNITS,
                    ]
                })
                .collect()
        })
        .collect()
}

fn distance(a: Point, b: Point) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn normalize(strokes: &[Vec<Point>]) -> Vec<Stroke> {
    let mut minimum = [f32::INFINITY; 2];
    let mut maximum = [f32::NEG_INFINITY; 2];
    for point in strokes.iter().flatten() {
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    let scale = (maximum[0] - minimum[0])
        .max(maximum[1] - minimum[1])
        .max(MIN_EXTENT);
    let center = [
        (minimum[0] + maximum[0]) * 0.5,
        (minimum[1] + maximum[1]) * 0.5,
    ];
    let axis_scale = std::array::from_fn::<_, 2, _>(|axis| {
        let extent = maximum[axis] - minimum[axis];
        // Expanding a thin axis amplifies pen wobble and changes stroke directions.
        if extent < THIN_AXIS * scale {
            scale
        } else {
            extent.max(MIN_EXTENT)
        }
    });
    strokes
        .iter()
        .map(|stroke| {
            let points: Vec<Point> = stroke
                .iter()
                .map(|point| {
                    [
                        (point[0] - center[0]) / axis_scale[0] + 0.5,
                        (point[1] - center[1]) / axis_scale[1] + 0.5,
                    ]
                })
                .collect();
            resample(&points)
        })
        .collect()
}

fn resample(points: &[Point]) -> Stroke {
    let mut lengths = vec![0.0; points.len()];
    for index in 1..points.len() {
        lengths[index] = lengths[index - 1] + distance(points[index - 1], points[index]);
    }
    let total = lengths[lengths.len() - 1];
    let mut sampled = [points[0]; SAMPLES];
    if total < EPSILON {
        return sampled;
    }
    let mut segment = 1;
    for (index, point) in sampled.iter_mut().enumerate().skip(1) {
        let target = total * index as f32 / (SAMPLES - 1) as f32;
        while segment < points.len() - 1 && lengths[segment] < target {
            segment += 1;
        }
        let fraction = (target - lengths[segment - 1])
            / (lengths[segment] - lengths[segment - 1]).max(EPSILON);
        *point = [
            points[segment - 1][0] + fraction * (points[segment][0] - points[segment - 1][0]),
            points[segment - 1][1] + fraction * (points[segment][1] - points[segment - 1][1]),
        ];
    }
    sampled
}

fn ink_signature(strokes: &[Stroke]) -> Ink {
    let mut distances = [u8::MAX; CELLS * 4];
    for stroke in strokes {
        for segment in stroke.windows(2) {
            let dx = segment[1][0] - segment[0][0];
            let dy = segment[1][1] - segment[0][1];
            let angle = dy.atan2(dx).rem_euclid(std::f32::consts::PI);
            let bin = ((angle * 4.0 / std::f32::consts::PI).round() as usize) % 4;
            let steps = (distance(segment[0], segment[1]) * GRID as f32 * INK_STEPS_PER_CELL)
                .ceil()
                .max(1.) as usize;
            for step in 0..=steps {
                let fraction = step as f32 / steps as f32;
                let pixel = |axis: usize| {
                    ((segment[0][axis] + fraction * (segment[1][axis] - segment[0][axis]))
                        * (GRID - 1) as f32)
                        .round()
                        .clamp(0., (GRID - 1) as f32) as usize
                };
                let pos = pixel(1) * GRID + pixel(0);
                for k in 0usize..4 {
                    let delta = k.abs_diff(bin).min(4 - k.abs_diff(bin));
                    distances[k * CELLS + pos] =
                        distances[k * CELLS + pos].min(ORIENTATION_COST[delta]);
                }
            }
        }
    }
    let pixels = distances
        .iter()
        .enumerate()
        .filter_map(|(i, &d)| (d == 0).then_some(i as u16))
        .collect();
    for k in 0usize..4 {
        let offset = k * CELLS;
        for i in 0..CELLS {
            if i % GRID > 0 {
                distances[offset + i] =
                    distances[offset + i].min(distances[offset + i - 1].saturating_add(1));
            }
            if i >= GRID {
                distances[offset + i] =
                    distances[offset + i].min(distances[offset + i - GRID].saturating_add(1));
            }
        }
        for i in (0..CELLS).rev() {
            if i % GRID + 1 < GRID {
                distances[offset + i] =
                    distances[offset + i].min(distances[offset + i + 1].saturating_add(1));
            }
            if i + GRID < CELLS {
                distances[offset + i] =
                    distances[offset + i].min(distances[offset + i + GRID].saturating_add(1));
            }
        }
    }
    Ink { pixels, distances }
}

fn directed_cost(from: &Ink, to: &Ink) -> f32 {
    from.pixels
        .iter()
        .map(|&i| to.distances[i as usize] as u32)
        .sum::<u32>() as f32
        / (from.pixels.len().max(1) * GRID) as f32
}

fn ink_cost(a: &Ink, b: &Ink) -> f32 {
    (directed_cost(a, b) + directed_cost(b, a)) / 2.0
}

fn stroke_cost(a: &Stroke, b: &Stroke) -> f32 {
    let ca: Point = std::array::from_fn(|k| a.iter().map(|p| p[k]).sum::<f32>() / SAMPLES as f32);
    let cb: Point = std::array::from_fn(|k| b.iter().map(|p| p[k]).sum::<f32>() / SAMPLES as f32);
    let mut forward = 0.;
    let mut reverse = 0.;
    for i in 0..SAMPLES {
        for k in 0..2 {
            forward += ((a[i][k] - ca[k]) - (b[i][k] - cb[k])).powi(2);
            reverse += ((a[i][k] - ca[k]) - (b[SAMPLES - 1 - i][k] - cb[k])).powi(2);
        }
    }
    let center = (ca[0] - cb[0]).powi(2) + (ca[1] - cb[1]).powi(2);
    ((forward.min(reverse) / SAMPLES as f32) + CENTER_WEIGHT * center).sqrt()
}
fn alignment_cost(a: &[Stroke], b: &[Stroke]) -> f32 {
    let size = a.len().max(b.len());
    debug_assert!(size <= MAX_STROKES);
    let unmatched =
        |stroke: &Stroke| UNMATCHED_COST + UNMATCHED_LENGTH_COST * stroke_length(stroke);
    let mut costs = vec![0.0; size * size];
    for i in 0..size {
        for j in 0..size {
            if i >= a.len() {
                costs[i * size + j] = unmatched(&b[j]);
            } else if j >= b.len() {
                costs[i * size + j] = unmatched(&a[i]).max(UNEXPLAINED_INK_COST);
            }
        }
    }
    for (i, stroke_a) in a.iter().enumerate() {
        for (j, stroke_b) in b.iter().enumerate() {
            costs[i * size + j] = stroke_cost(stroke_a, stroke_b);
        }
    }
    let mut row_cost = [0.0; MAX_STROKES + 1];
    let mut column_cost = [0.0; MAX_STROKES + 1];
    let mut assigned = [0usize; MAX_STROKES + 1];
    let mut previous = [0usize; MAX_STROKES + 1];
    for row in 1..=size {
        assigned[0] = row;
        let mut column = 0;
        let mut minimum = [f32::INFINITY; MAX_STROKES + 1];
        let mut used = [false; MAX_STROKES + 1];
        loop {
            used[column] = true;
            let current_row = assigned[column];
            let mut delta = f32::INFINITY;
            let mut next_column = 0;
            for candidate in 1..=size {
                if used[candidate] {
                    continue;
                }
                let cost = costs[(current_row - 1) * size + candidate - 1]
                    - row_cost[current_row]
                    - column_cost[candidate];
                if cost < minimum[candidate] {
                    minimum[candidate] = cost;
                    previous[candidate] = column;
                }
                if minimum[candidate] < delta {
                    delta = minimum[candidate];
                    next_column = candidate;
                }
            }
            for candidate in 0..=size {
                if used[candidate] {
                    row_cost[assigned[candidate]] += delta;
                    column_cost[candidate] -= delta;
                } else {
                    minimum[candidate] -= delta;
                }
            }
            column = next_column;
            if assigned[column] == 0 {
                break;
            }
        }
        loop {
            let next_column = previous[column];
            assigned[column] = assigned[next_column];
            column = next_column;
            if column == 0 {
                break;
            }
        }
    }
    (1..=size)
        .map(|column| costs[(assigned[column] - 1) * size + column - 1])
        .sum::<f32>()
        / size as f32
}

fn stroke_length(s: &Stroke) -> f32 {
    s.windows(2).map(|p| distance(p[0], p[1])).sum()
}

// Component expansion shifts coordinates more than ranks; midpoint ties keep straight lines thin.
fn equalize(mut strokes: Vec<Stroke>) -> Vec<Stroke> {
    // Arclength weights keep a split stroke from gaining influence.
    let mut weights = Vec::new();
    for stroke in &strokes {
        for i in 0..SAMPLES {
            let before = if i > 0 {
                distance(stroke[i - 1], stroke[i]) * 0.5
            } else {
                0.
            };
            let after = if i + 1 < SAMPLES {
                distance(stroke[i], stroke[i + 1]) * 0.5
            } else {
                0.
            };
            weights.push((before + after).max(MIN_WEIGHT));
        }
    }
    let axes: [Vec<(f32, f32)>; 2] = std::array::from_fn(|k| {
        let mut v: Vec<(f32, f32)> = strokes
            .iter()
            .flatten()
            .zip(&weights)
            .map(|(p, &w)| (p[k], w))
            .collect();
        v.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut total = 0.;
        for p in &mut v {
            total += p.1;
            p.1 = total;
        }
        v
    });
    let strengths: [f32; 2] =
        std::array::from_fn(|k| RANK_STRENGTH * (axes[k].last().unwrap().0 - axes[k][0].0));
    for p in strokes.iter_mut().flatten() {
        for k in 0..2 {
            let lo = axes[k].partition_point(|v| v.0 < p[k]);
            let hi = axes[k].partition_point(|v| v.0 <= p[k]);
            let before = if lo > 0 { axes[k][lo - 1].1 } else { 0. };
            let through = if hi > 0 { axes[k][hi - 1].1 } else { 0. };
            let rank = (before + through) * 0.5 / axes[k].last().unwrap().1;
            p[k] = (1.0 - strengths[k]) * p[k] + strengths[k] * rank;
        }
    }
    strokes
}
