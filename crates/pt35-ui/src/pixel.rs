//! Pixel-art marks drawn from a bitmap. Crisp at any whole-number scale, and
//! no icon theme needed.

use crate::canvas::Canvas;
use pt35_common::theme::Rgb;

/// The menu button's mark: the Weyland-Yutani W, its three notches cut out
/// with a one-pixel gap so they read in one colour. 19x8. Still called SKULL,
/// so this stays a one-file change against upstream.
pub const SKULL: &[&str] = &[
    "###.###.###.###.###",
    ".###.#.#####.#.###.",
    "..###..#####..###..",
    "...###.##.##.###...",
    "....#####.#####....",
    "....####.#.####....",
    "....###.###.###....",
    "....##.#####.##....",
];

/// The W's gold, the same as the fastfetch logo's.
pub const SKULL_GOLD: Rgb = Rgb(0xff, 0xcc, 0x00);

/// The launcher's mark for a pinned app, 5x8.
pub const PIN: &[&str] = &[
    ".###.", "#####", ".###.", ".###.", "#####", "..#..", "..#..", "..#..",
];

/// A binary PPM (`P6`, 8-bit), as `grim -t ppm` writes it: width, height and
/// RGB bytes.
pub fn parse_ppm(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut fields = Vec::with_capacity(4);
    let mut at = 0;
    while fields.len() < 4 {
        while data.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !data.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        fields.push(std::str::from_utf8(&data[start..at]).ok()?);
    }
    // Exactly one whitespace byte between the header and the pixels.
    at += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let len = (w as usize).checked_mul(h as usize)?.checked_mul(3)?;
    let pixels = data.get(at..at.checked_add(len)?)?;
    Some((w, h, pixels.to_vec()))
}

/// Width and height of `bitmap` drawn at `scale`.
pub fn size(bitmap: &[&str], scale: u32) -> (u32, u32) {
    let width = bitmap.iter().map(|row| row.len()).max().unwrap_or(0) as u32;
    (width * scale, bitmap.len() as u32 * scale)
}

/// Draw `bitmap` with its top-left corner at `x`, `y`.
pub fn draw(canvas: &mut Canvas, bitmap: &[&str], x: i32, y: i32, scale: u32, color: Rgb) {
    for (row, line) in bitmap.iter().enumerate() {
        for (column, cell) in line.bytes().enumerate() {
            if cell == b'#' {
                canvas.rect(
                    x + (column as u32 * scale) as i32,
                    y + (row as u32 * scale) as i32,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

/// Draw `bitmap` centred on `cx`, `cy`.
pub fn draw_centred(
    canvas: &mut Canvas,
    bitmap: &[&str],
    cx: i32,
    cy: i32,
    scale: u32,
    color: Rgb,
) {
    let (w, h) = size(bitmap, scale);
    draw(
        canvas,
        bitmap,
        cx - w as i32 / 2,
        cy - h as i32 / 2,
        scale,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ppm_grim_writes() {
        let mut data = b"P6\n2 1\n255\n".to_vec();
        data.extend([1, 2, 3, 4, 5, 6]);
        assert_eq!(parse_ppm(&data), Some((2, 1, vec![1, 2, 3, 4, 5, 6])));
        assert_eq!(parse_ppm(b"P6\n2 1\n255\n\x01"), None, "short");
        assert_eq!(parse_ppm(b"P3\n1 1\n255\n1 2 3"), None, "ascii ppm");
    }

    #[test]
    fn the_skull_is_a_rectangle_of_known_size() {
        assert!(SKULL.iter().all(|row| row.len() == 19));
        assert_eq!(size(SKULL, 1), (19, 8));
    }

    #[test]
    fn ink_lands_only_where_the_bitmap_says() {
        let mut canvas = Canvas::new(19, 8);
        let ink = Rgb(0xff, 0xff, 0xff);
        draw(&mut canvas, SKULL, 0, 0, 1, ink);
        assert_eq!(canvas.pixel(0, 0), ink.to_argb8888(), "top of the left arm");
        assert_ne!(
            canvas.pixel(3, 0),
            ink.to_argb8888(),
            "the gap round a notch"
        );
    }
}
