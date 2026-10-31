//! The oracle: every (Y, U, V) against gliff-sw, and the HLSL constants.

use super::INVERSE;
use super::PRESENT_HLSL;
use super::hlsl_line;
use super::ps_yuv;

#[test]
fn the_hlsl_carries_gliff_sws_constants() {
    for (name, value) in INVERSE {
        let line = hlsl_line(name, value);
        assert!(
            PRESENT_HLSL.lines().any(|l| l.trim() == line),
            "present.hlsl lacks `{line}`"
        );
    }
}

/// All 2^24 triples, each once through `i420_to_bgra` (a 2x2 chroma block
/// holds four Y): per U, a 128x512 picture whose chroma row is V and whose four
/// pixels under chroma column c are Y = 4c..4c+3.
#[test]
fn every_triple_is_within_one_code_of_gliff_sw() {
    let (width, height) = (128usize, 512usize);
    let mut y = vec![0u8; width * height];
    for row in 0..height {
        for col in 0..width {
            y[row * width + col] = ((col / 2) * 4 + (row % 2) * 2 + col % 2) as u8;
        }
    }
    let v: Vec<u8> = (0..height / 2)
        .flat_map(|row| std::iter::repeat_n(row as u8, width / 2))
        .collect();
    let (mut same, mut one, mut worst) = (0u64, 0u64, 0u8);
    for u in 0..=255u8 {
        let us = vec![u; width / 2 * height / 2];
        let bgra = gliff_sw::convert::i420_to_bgra(
            &y,
            &us,
            &v,
            (width, width / 2, width / 2),
            width,
            height,
        );
        for (i, px) in bgra.as_chunks::<4>().0.iter().enumerate() {
            let (row, col) = (i / width, i % width);
            let got = ps_yuv(y[i], u, v[row / 2 * width / 2 + col / 2]);
            for (gpu, cpu) in got.iter().zip([px[2], px[1], px[0]]) {
                let d = gpu.abs_diff(cpu);
                worst = worst.max(d);
                match d {
                    0 => same += 1,
                    1 => one += 1,
                    _ => {}
                }
            }
        }
    }
    let total = 3 * (1u64 << 24);
    assert!(
        worst <= 1,
        "max difference {worst}; {one} channels off by one"
    );
    assert_eq!(same + one, total, "every channel within one code");
    println!("{same} channels equal, {one} off by one, of {total}");
}
