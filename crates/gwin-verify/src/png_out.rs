//! Writes a decoded BGRA picture as an RGB PNG.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use gliff_sw::BgraFrame;

pub fn write_png(path: &Path, frame: &BgraFrame) -> anyhow::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|px| [px[2], px[1], px[0]])
        .collect();
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgb)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use gliff_sw::BgraFrame;

    use super::write_png;

    #[test]
    fn a_picture_round_trips_through_png() {
        let frame = BgraFrame {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let path = std::env::temp_dir().join(format!("gwv-png-{}.png", std::process::id()));
        write_png(&path, &frame).expect("write");
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&path).expect("open"),
        ));
        let mut reader = decoder.read_info().expect("info");
        let buf_len = reader.output_buffer_size().expect("size fits in memory");
        let mut buf = vec![0; buf_len];
        let info = reader.next_frame(&mut buf).expect("frame");
        assert_eq!((info.width, info.height), (2, 1), "size");
        assert_eq!(
            &buf[..6],
            &[0, 0, 255, 255, 0, 0],
            "BGRA blue then red as RGB"
        );
        std::fs::remove_file(&path).expect("clean up");
    }
}
