//! Reproducible offline metrics and monochrome fixtures. No runtime debug mode.
use bevy_math::{UVec2, Vec2};
use puzzella_puzzle::{
    fingerprint::{
        decode_v4_reference, sample_profile, worst_case_profiles, EdgeFingerprint,
        EdgeSilhouetteDescriptor,
    },
    procedural::{
        decode_profile, piece_profiles, raw_profile, sd_tab, EdgeId, EdgeOrientation, EdgeProfile,
    },
};
use std::{collections::HashSet, fmt::Write, path::Path};

fn svg(width: u32, height: u32, title: &str) -> String {
    format!("<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' viewBox='0 0 {width} {height}'><rect width='100%' height='100%' fill='#faf9f6'/><text x='20' y='28' font-family='sans-serif' font-size='20' fill='#26343d'>{title}</text>")
}
/// Follow the zero contour by horizontal sections. Offline bisection only;
/// gameplay and WGSL use the unchanged closed-form SDF.
fn contour(p: EdgeProfile) -> Vec<Vec2> {
    let mut left = Vec::new();
    let mut right = Vec::new();
    for row in 0..=160 {
        let y = if row == 0 {
            0.00001
        } else {
            p.depth * 100.0 * 0.98 * row as f32 / 160.0
        };
        // The head center is inside every head section; the stem center is inside below it.
        let center = (p.center
            + p.asymmetry * p.head_width * if y > p.depth * 100.0 * 0.51 { 1.0 } else { 0.0 })
            * 100.0;
        let mut limits = [0.0; 2];
        for (index, sign) in [-1.0, 1.0].into_iter().enumerate() {
            let mut inside = center;
            let mut outside = center + sign * 100.0;
            for _ in 0..28 {
                let x = (inside + outside) * 0.5;
                if sd_tab(Vec2::new(x, y), p, 100.0, 100.0) <= 0.0 {
                    inside = x;
                } else {
                    outside = x;
                }
            }
            limits[index] = (inside + outside) * 0.5;
        }
        left.push(Vec2::new(limits[0], y * p.polarity));
        right.push(Vec2::new(limits[1], y * p.polarity));
    }
    let mut points = vec![Vec2::ZERO];
    points.extend(left);
    points.extend(right.into_iter().rev());
    points.push(Vec2::new(100.0, 0.0));
    points
}
fn path(points: impl IntoIterator<Item = Vec2>) -> String {
    let mut out = String::new();
    for (i, p) in points.into_iter().enumerate() {
        write!(
            out,
            "{} {:.4} {:.4} ",
            if i == 0 { "M" } else { "L" },
            p.x,
            p.y
        )
        .unwrap();
    }
    out
}
fn edge_panel(svg: &mut String, raw: [u32; 2], x: f32, y: f32, label: &str, blank: bool, v4: bool) {
    let mut p = if v4 {
        decode_v4_reference(raw)
    } else {
        decode_profile(raw)
    };
    p.polarity = if blank { -1.0 } else { 1.0 };
    let points = contour(p)
        .into_iter()
        .map(|q| Vec2::new(x + q.x * 2.0, y - q.y * 2.0))
        .chain([Vec2::new(x + 200.0, y + 45.0), Vec2::new(x, y + 45.0)]);
    write!(svg,"<path d='{}Z' fill='#c7c7c7' stroke='#26343d' stroke-width='1'/><text x='{x}' y='{}' font-family='sans-serif' font-size='12' fill='#26343d'>{label}</text>",path(points),y+62.0).unwrap();
}
fn nearest(descriptors: &[EdgeSilhouetteDescriptor]) -> (Vec<u32>, Vec<f64>, Vec<usize>) {
    let mut distances = Vec::new();
    let mut similarities = Vec::new();
    let mut neighbors = Vec::new();
    for (i, a) in descriptors.iter().enumerate() {
        let (j, d) = descriptors
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, b)| (j, a.hamming(b)))
            .min_by_key(|&(j, d)| (d, j))
            .unwrap();
        distances.push(d);
        similarities.push(a.iou(&descriptors[j]));
        neighbors.push(j);
    }
    (distances, similarities, neighbors)
}
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "target".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let edges: Vec<_> = (0..100_000)
        .map(|i| {
            let edge = EdgeId {
                orientation: if i % 2 == 0 {
                    EdgeOrientation::Horizontal
                } else {
                    EdgeOrientation::Vertical
                },
                x: (i / 2) % 999 + 1,
                y: (i / 2) / 999 + 1,
            };
            assert!(edge.x > 0 && edge.x < 1000 && edge.y > 0 && edge.y < 1000);
            (edge, raw_profile(42, edge))
        })
        .collect();
    let mut distinct = HashSet::new();
    let counts = [6, 7, 4, 4, 4, 4, 7];
    let names = ["style", "center", "width", "depth", "neck", "head", "skew"];
    let mut histogram: Vec<_> = counts.iter().map(|&n| vec![0; n]).collect();
    for &(_, raw) in &edges {
        let f = EdgeFingerprint::from_raw(raw);
        distinct.insert(f);
        for (axis, c) in f.classes().into_iter().enumerate() {
            histogram[axis][c as usize] += 1;
        }
    }
    let mut report = format!(
        "seed=42, edges=100000, theoretical=75264, distinct={}\n",
        distinct.len()
    );
    let mut csv = String::from("axis,class,count\n");
    for (axis, bins) in histogram.iter().enumerate() {
        writeln!(report, "{}: {bins:?}", names[axis]).unwrap();
        for (class, n) in bins.iter().enumerate() {
            writeln!(csv, "{},{class},{n}", names[axis]).unwrap();
        }
    }
    std::fs::write(dir.join("edge-fingerprint-histogram.csv"), csv).unwrap();
    // Actual byte-quantized intervals, with the other five samples fixed at 128.
    let mut values = String::from("style,axis,class,min,max,other_samples\n");
    for style in 1..=6 {
        for axis in 0..6 {
            let count = counts[axis + 1];
            let mut ranges = vec![(f32::INFINITY, f32::NEG_INFINITY); count];
            for sample in 0..256 {
                let mut samples = [128; 6];
                samples[axis] = sample;
                let raw = sample_profile(style, samples);
                let p = decode_profile(raw);
                let class = EdgeFingerprint::from_raw(raw).classes()[axis + 1] as usize;
                let v = [
                    p.center,
                    p.width,
                    p.depth,
                    p.neck_width / p.head_width,
                    p.head_width / p.width,
                    p.asymmetry,
                ][axis];
                ranges[class].0 = ranges[class].0.min(v);
                ranges[class].1 = ranges[class].1.max(v);
            }
            for (class, (min, max)) in ranges.into_iter().enumerate() {
                writeln!(
                    values,
                    "{style},{},{class},{min:.8},{max:.8},128",
                    names[axis + 1]
                )
                .unwrap();
            }
        }
    }
    std::fs::write(dir.join("edge-fingerprint-class-values.csv"), values).unwrap();
    let mut axes = svg(
        1680,
        970,
        "One axis at a time · other samples fixed at 128 · v5",
    );
    for (axis, &count) in counts.iter().enumerate() {
        for class in 0..count {
            let mut samples = [128; 6];
            let style = if axis == 0 { class as u32 + 1 } else { 1 };
            if axis > 0 {
                samples[axis - 1] = if count == 7 {
                    [0, 43, 85, 128, 170, 213, 255][class]
                } else {
                    [0, 85, 170, 255][class]
                };
            }
            edge_panel(
                &mut axes,
                sample_profile(style, samples),
                20.0 + class as f32 * 235.0,
                100.0 + axis as f32 * 128.0,
                &format!("{} class {class}", names[axis]),
                false,
                false,
            );
        }
    }
    axes.push_str("</svg>");
    std::fs::write(dir.join("edge-fingerprint-classes.svg"), axes).unwrap();
    // Same 1024 distinct EdgeIds for both versions, polarity canonicalized only.
    let raws: Vec<_> = edges.iter().take(1024).map(|&(_, raw)| raw).collect();
    let v4: Vec<_> = raws
        .iter()
        .map(|&r| EdgeSilhouetteDescriptor::v4_reference(r))
        .collect();
    let v5: Vec<_> = raws
        .iter()
        .map(|&r| EdgeSilhouetteDescriptor::from_raw(r))
        .collect();
    let mut nearest_csv =
        String::from("version,edge,nearest_edge,hamming,normalized_hamming,iou\n");
    let mut closest_svg = svg(
        960,
        570,
        "Nearest distinct EdgeId silhouettes: v4 / v5 (same candidate set)",
    );
    for (version, descriptors) in [(4, &v4), (5, &v5)] {
        let (d, ious, neighbors) = nearest(descriptors);
        let mut sorted = d.clone();
        sorted.sort_unstable();
        let mean = d.iter().map(|&x| x as f64).sum::<f64>() / d.len() as f64;
        let mean_iou = ious.iter().sum::<f64>() / ious.len() as f64;
        writeln!(report,"v{version}, descriptors={}, nearest Hamming mean={mean:.4}, p10={}, median={}, zero={}, nearest IoU mean={mean_iou:.6}",d.len(),sorted[d.len()/10],sorted[d.len()/2],d.iter().filter(|&&n| n==0).count()).unwrap();
        for i in 0..d.len() {
            writeln!(
                nearest_csv,
                "{version},{i},{},{},{:.6},{:.6}",
                neighbors[i],
                d[i],
                d[i] as f64 / 2048.0,
                ious[i]
            )
            .unwrap();
        }
        // Deliberately difficult examples: four closest pairs, skipping duplicate reversals.
        let mut order: Vec<_> = (0..d.len()).collect();
        order.sort_by_key(|&i| (d[i], i));
        let mut seen = HashSet::new();
        let mut row = 0;
        for i in order {
            let j = neighbors[i];
            if !seen.insert((i.min(j), i.max(j))) {
                continue;
            }
            let x = if version == 4 { 20.0 } else { 500.0 };
            let y = 95.0 + row as f32 * 118.0;
            edge_panel(
                &mut closest_svg,
                raws[i],
                x,
                y,
                &format!("v{version} edge {i} / Hamming {}", d[i]),
                false,
                version == 4,
            );
            edge_panel(
                &mut closest_svg,
                raws[j],
                x + 225.0,
                y,
                &format!("neighbor {j} / IoU {:.3}", ious[i]),
                false,
                version == 4,
            );
            row += 1;
            if row == 4 {
                break;
            }
        }
    }
    closest_svg.push_str("</svg>");
    std::fs::write(dir.join("edge-fingerprint-nearest.svg"), closest_svg).unwrap();
    std::fs::write(dir.join("edge-fingerprint-nearest.csv"), nearest_csv).unwrap();
    // Four intentionally different exemplars per style, found in real hashed edges.
    let mut selected = Vec::new();
    for style in 0..6 {
        for case in 0..4 {
            let &(_, raw) = edges
                .iter()
                .find(|&&(_, raw)| {
                    let f = EdgeFingerprint::from_raw(raw);
                    f.style == style
                        && match case {
                            0 => f.center_class == 0 && f.skew_class == 0 && f.depth_class == 0,
                            1 => f.center_class == 6 && f.skew_class == 6 && f.depth_class == 3,
                            2 => f.center_class == 3 && f.neck_class == 0 && f.head_class == 3,
                            _ => f.center_class == 3 && f.neck_class == 3 && f.head_class == 0,
                        }
                })
                .unwrap();
            selected.push(raw);
        }
    }
    let mut matching = svg(
        1460,
        1135,
        "Shape-only matching: 24 tabs / shuffled complementary blanks (v5)",
    );
    let mut key = String::from("tab,blank,raw0,raw1,style,center,width,depth,neck,head,skew\n");
    // A coprime permutation, independent of any shape parameter.
    for i in 0..24 {
        let x = 20.0 + (i % 3) as f32 * 230.0;
        let y = 100.0 + (i / 3) as f32 * 132.0;
        edge_panel(
            &mut matching,
            selected[i],
            x,
            y,
            &format!("A{}", i + 1),
            false,
            false,
        );
        let j = (i * 7 + 11) % 24;
        edge_panel(
            &mut matching,
            selected[j],
            x + 740.0,
            y,
            &format!("B{}", i + 1),
            true,
            false,
        );
        let f = EdgeFingerprint::from_raw(selected[j]);
        let c = f.classes();
        writeln!(
            key,
            "A{},B{},{},{},{},{},{},{},{},{},{}",
            j + 1,
            i + 1,
            selected[j][0],
            selected[j][1],
            c[0],
            c[1],
            c[2],
            c[3],
            c[4],
            c[5],
            c[6]
        )
        .unwrap();
    }
    matching.push_str("</svg>");
    std::fs::write(dir.join("edge-fingerprint-preview.svg"), matching).unwrap();
    std::fs::write(dir.join("edge-fingerprint-answer-key.csv"), key).unwrap();
    let mut worst = svg(
        1440,
        875,
        "Worst cases: extreme center / widest width and head / strongest lean (v5)",
    );
    for style in 1..=6 {
        for case in 0..6 {
            let samples = [
                if case % 2 == 0 { 0 } else { 255 },
                255,
                if case < 2 { 0 } else { 255 },
                if case < 4 { 0 } else { 255 },
                255,
                if case % 2 == 0 { 0 } else { 255 },
            ];
            let raw = sample_profile(style, samples);
            edge_panel(
                &mut worst,
                raw,
                20.0 + case as f32 * 235.0,
                100.0 + (style - 1) as f32 * 128.0,
                &format!(
                    "{:?}: c{} d{} n{} s{}",
                    decode_profile(raw).style,
                    samples[0],
                    samples[2],
                    samples[3],
                    samples[5]
                ),
                case >= 4,
                false,
            );
        }
    }
    worst.push_str("</svg>");
    std::fs::write(dir.join("edge-fingerprint-worst.svg"), worst).unwrap();
    // Full 40x25 assembled puzzle: same fill on every piece, actual shared contours.
    let grid = UVec2::new(40, 25);
    let mut puzzle = svg(
        1640,
        1070,
        "1000 monochrome pieces · seed 42 · generator v5",
    );
    for y in 0..grid.y {
        for x in 0..grid.x {
            let profiles = piece_profiles(42, grid, UVec2::new(x, y));
            let mut points = Vec::new();
            for (side, raw) in profiles.into_iter().enumerate() {
                let mut edge = if raw[0] == 0 {
                    vec![Vec2::ZERO, Vec2::new(100.0, 0.0)]
                } else {
                    contour(decode_profile(raw))
                };
                if side >= 2 {
                    edge.reverse();
                }
                points.extend(edge.into_iter().map(|q| {
                    let local = match side {
                        0 => Vec2::new(q.x, -q.y),
                        1 => Vec2::new(100.0 + q.y, q.x),
                        2 => Vec2::new(q.x, 100.0 - q.y),
                        _ => Vec2::new(q.y, q.x),
                    };
                    Vec2::new(20.0 + x as f32 * 40.0, 50.0 + y as f32 * 40.0) + local * 0.4
                }));
            }
            write!(
                puzzle,
                "<path d='{}Z' fill='#c7c7c7' stroke='#26343d' stroke-width='0.55'/>",
                path(points)
            )
            .unwrap();
        }
    }
    puzzle.push_str("</svg>");
    std::fs::write(dir.join("edge-fingerprint-puzzle.svg"), puzzle).unwrap();
    writeln!(
        report,
        "worst-case profiles tested={}, matching pairs=24, monochrome pieces=1000",
        worst_case_profiles().len()
    )
    .unwrap();
    print!("{report}");
    std::fs::write(dir.join("edge-fingerprint-metrics.txt"), report).unwrap();
}
