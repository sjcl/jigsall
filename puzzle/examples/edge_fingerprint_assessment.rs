//! Unbiased matching fixtures, aspect metrics, class ablations, and an offline human tool.
use bevy_math::Vec2;
use puzzella_puzzle::fingerprint::{assessment::*, worst_case_profiles, EdgeFingerprint};
use std::{collections::HashMap, fmt::Write, path::Path};

fn svg_path(points: impl IntoIterator<Item = Vec2>) -> String {
    let mut s = String::new();
    for (i, p) in points.into_iter().enumerate() {
        write!(
            s,
            "{} {:.3} {:.3} ",
            if i == 0 { "M" } else { "L" },
            p.x,
            p.y
        )
        .unwrap();
    }
    s
}
fn panel(raw: [u32; 2], length: f32, short: f32, blank: bool) -> String {
    let scale = 200.0 / length;
    let sign = if blank { 1.0 } else { -1.0 };
    svg_path(
        tab_contour(raw, length, short, 160)
            .into_iter()
            .map(|p| Vec2::new(p.x * scale, 48.0 + sign * p.y * scale))
            .chain([Vec2::new(200.0, 94.0), Vec2::new(0.0, 94.0)]),
    ) + "Z"
}
fn random_matching(dir: &Path) {
    let mut manifest = String::from("seed,target,blank,orientation,edge_x,edge_y,raw0,raw1\n");
    for seed in SEEDS {
        let edges = random_edges(seed, 0, 32, None);
        let order = blank_permutation(seed, 0, 32);
        let mut svg=format!("<svg xmlns='http://www.w3.org/2000/svg' width='1900' height='1095' viewBox='0 0 1900 1095'><rect width='100%' height='100%' fill='#faf9f6'/><g font-family='sans-serif' fill='#26343d'><text x='20' y='28' font-size='20'>Uniform random matching · generator v5 · seed {seed} · 32 pairs</text><text x='20' y='50'>No style / class / silhouette filtering · blanks shuffled with a separate RNG</text></g>");
        for (i, &j) in order.iter().enumerate() {
            let x = 20.0 + (i % 4) as f32 * 235.0;
            let y = 70.0 + (i / 4) as f32 * 126.0;
            for (edge, blank, px, label) in [
                (&edges[i], false, x, format!("A{}", i + 1)),
                (&edges[j], true, x + 950.0, format!("B{}", i + 1)),
            ] {
                write!(svg,"<g transform='translate({px} {y})'><path d='{}' fill='#c7c7c7' stroke='#26343d'/><text x='0' y='113' font-family='sans-serif' font-size='12'>{label}</text></g>",panel(edge.raw,100.0,100.0,blank)).unwrap();
            }
            let e = edges[j];
            writeln!(
                manifest,
                "{seed},A{},B{},{},{},{},{},{}",
                j + 1,
                i + 1,
                orientation_name(e.id.orientation),
                e.id.x,
                e.id.y,
                e.raw[0],
                e.raw[1]
            )
            .unwrap();
        }
        svg.push_str("</svg>");
        std::fs::write(dir.join(format!("random-matching-{seed}.svg")), svg).unwrap();
    }
    std::fs::write(dir.join("random-matching-manifest.csv"), manifest).unwrap();
}
#[derive(Clone)]
struct Nearest {
    distances: Vec<u32>,
    ious: Vec<f64>,
}
fn nearest(masks: &[SilhouetteMask]) -> Nearest {
    let mut distances = Vec::new();
    let mut ious = Vec::new();
    for (i, a) in masks.iter().enumerate() {
        let (j, d) = masks
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(j, b)| (j, a.hamming(b)))
            .min_by_key(|&(j, d)| (d, j))
            .unwrap();
        distances.push(d);
        ious.push(a.iou(&masks[j]));
    }
    Nearest { distances, ious }
}
fn stats(d: &[u32]) -> (f64, u32, u32, u32, usize) {
    let mut sorted = d.to_vec();
    sorted.sort_unstable();
    (
        d.iter().map(|&v| v as f64).sum::<f64>() / d.len() as f64,
        sorted[d.len() / 10],
        sorted[d.len() / 2],
        sorted[0],
        d.iter().filter(|&&v| v == 0).count(),
    )
}
fn aspect_metrics(dir: &Path) {
    let mut csv=String::from("seed,aspect,orientation,length,short,version,raster,edges,mean_hamming,p10,median,min,zero_count,mean_normalized_hamming,mean_iou\n");
    let mut cache = HashMap::new();
    let mut grouped = HashMap::new();
    for seed in SEEDS {
        for o in ORIENTATIONS {
            let edges = random_edges(seed, 0x4d45_5452_4943, 512, Some(o));
            for (aspect, size) in ASPECTS {
                let (length, short) = edge_geometry(size, o);
                let ratio = (length / short) as u32;
                for mode in [RasterMode::Normalized64, RasterMode::Display64] {
                    for version in [4, 5] {
                        let n = cache
                            .entry((seed, orientation_name(o), ratio, mode, version))
                            .or_insert_with(|| {
                                let masks: Vec<_> = edges
                                    .iter()
                                    .map(|e| {
                                        SilhouetteMask::raster(
                                            e.raw,
                                            length,
                                            short,
                                            mode,
                                            version == 4,
                                        )
                                    })
                                    .collect();
                                nearest(&masks)
                            });
                        let (mean, p10, median, min, zeros) = stats(&n.distances);
                        let group = grouped
                            .entry((aspect, orientation_name(o), mode, version))
                            .or_insert_with(|| (Vec::new(), Vec::new()));
                        group.0.extend_from_slice(&n.distances);
                        group.1.extend_from_slice(&n.ious);
                        let (w, h) = mode.dimensions();
                        writeln!(csv,"{seed},{aspect},{},{length},{short},{version},{},{},{mean:.6},{p10},{median},{min},{zeros},{:.8},{:.8}",orientation_name(o),mode.name(),edges.len(),mean/(w*h) as f64,n.ious.iter().sum::<f64>()/n.ious.len() as f64).unwrap();
                    }
                }
            }
            println!(
                "nearest metrics: seed {seed}, {} complete",
                orientation_name(o)
            );
        }
    }
    for (aspect, size) in ASPECTS {
        for o in ORIENTATIONS {
            let (length, short) = edge_geometry(size, o);
            for mode in [RasterMode::Normalized64, RasterMode::Display64] {
                for version in [4, 5] {
                    let (d, ious) = &grouped[&(aspect, orientation_name(o), mode, version)];
                    let (mean, p10, median, min, zeros) = stats(d);
                    let (w, h) = mode.dimensions();
                    writeln!(csv,"all,{aspect},{},{length},{short},{version},{},{},{mean:.6},{p10},{median},{min},{zeros},{:.8},{:.8}",orientation_name(o),mode.name(),d.len(),mean/(w*h) as f64,ious.iter().sum::<f64>()/ious.len() as f64).unwrap();
                }
            }
        }
    }
    std::fs::write(dir.join("aspect-silhouette-metrics.csv"), csv).unwrap();
}
#[derive(Default)]
struct MaskCache {
    lookup: HashMap<(u32, u32, u32, RasterMode), usize>,
    masks: Vec<SilhouetteMask>,
}
impl MaskCache {
    fn get(&mut self, raw: [u32; 2], ratio: u32, mode: RasterMode) -> usize {
        let key = (raw[0] & !8, raw[1], ratio, mode);
        if let Some(&index) = self.lookup.get(&key) {
            return index;
        }
        let index = self.masks.len();
        self.masks.push(SilhouetteMask::raster(
            raw,
            100.0 * ratio as f32,
            100.0,
            mode,
            false,
        ));
        self.lookup.insert(key, index);
        index
    }
}
#[derive(Default)]
struct Separation {
    distances: Vec<u32>,
    min_pair: Option<([u32; 2], [u32; 2])>,
    minimum: u32,
}
impl Separation {
    fn push(&mut self, d: u32, a: [u32; 2], b: [u32; 2]) {
        if self.min_pair.is_none() || d < self.minimum {
            self.minimum = d;
            self.min_pair = Some((a, b));
        }
        self.distances.push(d);
    }
}
fn axis_metrics(dir: &Path) {
    let normal: Vec<_> = SEEDS
        .into_iter()
        .flat_map(|seed| {
            random_edges(seed, 0x4158_4953, 16, None)
                .into_iter()
                .map(|e| e.raw)
        })
        .collect();
    let worst = worst_case_profiles();
    let mut cache = MaskCache::default();
    let mut summaries = HashMap::new();
    for (population, contexts) in [("random", normal), ("worst", worst)] {
        for ratio in [1, 2, 4] {
            for mode in [
                RasterMode::Normalized64,
                RasterMode::Display64,
                RasterMode::Display256,
            ] {
                for context in &contexts {
                    for (axis, &(_, count)) in AXES.iter().enumerate() {
                        for class in 0..count - 1 {
                            let a = replace_class(*context, axis, class);
                            let b = replace_class(*context, axis, class + 1);
                            let ia = cache.get(a, ratio, mode);
                            let ib = cache.get(b, ratio, mode);
                            let d = cache.masks[ia].hamming(&cache.masks[ib]);
                            for boundary in [class as i32, -1] {
                                summaries
                                    .entry((population, ratio, mode, axis, boundary))
                                    .or_insert_with(Separation::default)
                                    .push(d, a, b);
                            }
                        }
                    }
                }
                println!(
                    "class contribution: {population}, ratio {ratio}, {} complete",
                    mode.name()
                );
            }
        }
    }
    let mut csv=String::from("population,aspect,orientation,length,short,raster,axis,from_class,to_class,cases,mean_hamming,p10,median,min,zero_count,mean_normalized_hamming,min_raw0,min_raw1,min_changed_raw0,min_changed_raw1\n");
    let mut worst_svg=String::from("<svg xmlns='http://www.w3.org/2000/svg' width='1450' height='855' viewBox='0 0 1450 855'><rect width='100%' height='100%' fill='#faf9f6'/><text x='20' y='28' font-family='sans-serif' font-size='20'>Worst adjacent-class separation · actual v5 SDF · before / after</text>");
    for (aspect, size) in ASPECTS {
        for o in ORIENTATIONS {
            let (length, short) = edge_geometry(size, o);
            let ratio = (length / short) as u32;
            for population in ["random", "worst"] {
                for mode in [
                    RasterMode::Normalized64,
                    RasterMode::Display64,
                    RasterMode::Display256,
                ] {
                    for (axis, &(name, count)) in AXES.iter().enumerate() {
                        for boundary in -1..(count - 1) as i32 {
                            let s = &summaries[&(population, ratio, mode, axis, boundary)];
                            let (mean, p10, median, min, zeros) = stats(&s.distances);
                            let (w, h) = mode.dimensions();
                            let (a, b) = s.min_pair.unwrap();
                            writeln!(csv,"{population},{aspect},{},{length},{short},{},{name},{boundary},{},{},{mean:.6},{p10},{median},{min},{zeros},{:.8},{},{},{},{}",orientation_name(o),mode.name(),if boundary<0 {-1} else {boundary+1},s.distances.len(),mean/(w*h) as f64,a[0],a[1],b[0],b[1]).unwrap();
                        }
                    }
                }
            }
        }
    }
    for ratio in [1, 2, 4] {
        for (axis, &(name, _)) in AXES.iter().enumerate() {
            let s = &summaries[&("worst", ratio, RasterMode::Display64, axis, -1)];
            let (a, b) = s.min_pair.unwrap();
            let (_, _, _, min, zeros) = stats(&s.distances);
            let x = 20.0 + ((axis % 3) as f32) * 475.0;
            let y = 85.0 + ((ratio.ilog2() * 2 + axis as u32 / 3) as f32) * 125.0;
            for (raw, px) in [(a, x), (b, x + 215.0)] {
                write!(worst_svg,"<g transform='translate({px} {y})'><path d='{}' fill='#c7c7c7' stroke='#26343d'/></g>",panel(raw,100.0*ratio as f32,100.0,false)).unwrap();
            }
            write!(worst_svg,"<text x='{x}' y='{}' font-family='sans-serif' font-size='12'>{name}, edge/short={ratio}, display64 min={min}, zero={zeros}</text>",y+112.0).unwrap();
        }
    }
    worst_svg.push_str("</svg>");
    std::fs::write(dir.join("worst-class-separation.svg"), worst_svg).unwrap();
    std::fs::write(dir.join("axis-contribution.csv"), csv).unwrap();
}
fn human_tool(dir: &Path) {
    // 4 seeds x 2 orientations x 64 uniformly drawn edges, each rendered at all
    // three edge/short ratios. Rounds/candidate permutations are frozen independently.
    let mut datasets = String::from("[");
    for (si, seed) in SEEDS.into_iter().enumerate() {
        for (oi, o) in ORIENTATIONS.into_iter().enumerate() {
            if si > 0 || oi > 0 {
                datasets.push(',');
            }
            let edges = random_edges(seed, 0x0048_554d_414e, 64, Some(o));
            write!(
                datasets,
                "{{\"seed\":\"{seed}\",\"orientation\":\"{}\",\"edges\":[",
                orientation_name(o)
            )
            .unwrap();
            for (i, e) in edges.iter().enumerate() {
                if i > 0 {
                    datasets.push(',');
                }
                let classes = EdgeFingerprint::from_raw(e.raw).classes();
                write!(
                    datasets,
                    "{{\"x\":{},\"y\":{},\"raw\":[{},{}],\"classes\":[{}],\"contours\":[",
                    e.id.x,
                    e.id.y,
                    e.raw[0],
                    e.raw[1],
                    classes.map(|c| c.to_string()).join(",")
                )
                .unwrap();
                for (r, ratio) in [1, 2, 4].into_iter().enumerate() {
                    if r > 0 {
                        datasets.push(',');
                    }
                    datasets.push('[');
                    let length = ratio as f32 * 100.0;
                    for (j, p) in tab_contour(e.raw, length, 100.0, 128)
                        .into_iter()
                        .enumerate()
                    {
                        if j > 0 {
                            datasets.push(',');
                        }
                        write!(
                            datasets,
                            "[{:.3},{:.3}]",
                            p.x * 200.0 / length,
                            p.y * 200.0 / length
                        )
                        .unwrap();
                    }
                    datasets.push(']');
                }
                datasets.push_str("],\"display64\":[");
                for (r, ratio) in [1, 2, 4].into_iter().enumerate() {
                    if r > 0 {
                        datasets.push(',');
                    }
                    let mask = SilhouetteMask::raster(
                        e.raw,
                        ratio as f32 * 100.0,
                        100.0,
                        RasterMode::Display64,
                        false,
                    );
                    write!(
                        datasets,
                        "\"{}\"",
                        mask.words
                            .iter()
                            .map(|w| format!("{w:016x}"))
                            .collect::<String>()
                    )
                    .unwrap();
                }
                datasets.push_str("]}");
            }
            datasets.push_str("],\"rounds\":[");
            for round in 0..20 {
                if round > 0 {
                    datasets.push(',');
                }
                let target = trial_indices(seed, round, 64, 12).0;
                write!(datasets, "{{\"target\":{target},\"candidates\":{{").unwrap();
                for (i, count) in [8, 10, 12].into_iter().enumerate() {
                    if i > 0 {
                        datasets.push(',');
                    }
                    let (answer, indices) = trial_indices(seed, round, 64, count);
                    assert_eq!(answer, target);
                    write!(
                        datasets,
                        "\"{count}\":[{}]",
                        indices
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                    .unwrap();
                }
                datasets.push_str("}}");
            }
            datasets.push_str("]}");
        }
    }
    datasets.push(']');
    let html = include_str!("support/edge_matching.html").replace("__MATCHING_DATA__", &datasets);
    std::fs::write(dir.join("edge-matching-tool.html"), html).unwrap();
}
fn main() {
    assert_eq!(puzzella_core::GENERATOR_VERSION, 5);
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/edge-assessment".into());
    let dir = Path::new(&output);
    std::fs::create_dir_all(dir).unwrap();
    if !std::env::args().any(|arg| arg == "--tool-only") {
        random_matching(dir);
        aspect_metrics(dir);
        axis_metrics(dir);
    }
    human_tool(dir);
    println!("Wrote requested assessment artifacts to {}", dir.display());
}
