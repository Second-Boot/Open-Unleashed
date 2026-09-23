//! The vertical extent of a string in an `.ffn` font, as the HUD's text measure 0x431400 takes it:
//! the height of `"abcdefghijklmnopqrstuvwxyz"` (0x5CD0D4), top of the highest glyph box to the bottom
//! of the lowest, plus the font's line height for comparison.
//!
//! `cargo run -q -p nfs-formats --example fntextent -- <font.ffn> [text]`
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = std::fs::read(&a[1]).unwrap();
    let f = nfs_formats::fnt::Font::parse(&d).unwrap();
    let text = a.get(2).cloned().unwrap_or_else(|| "abcdefghijklmnopqrstuvwxyz".into());
    let (mut top, mut bottom) = (i32::MAX, i32::MIN);
    for c in text.chars() {
        if let Some(g) = f.glyph(c) {
            top = top.min(g.offset_y as i32);
            bottom = bottom.max(g.offset_y as i32 + g.height as i32);
        }
    }
    println!("{}: line {} extent {}..{} = {}", a[1], f.line_height, top, bottom, bottom - top);
}
