fn main() {
    let p = std::env::args().nth(1).unwrap();
    let d = std::fs::read(&p).unwrap();
    let f = nfs_formats::fnt::Font::parse(&d).unwrap();
    let a = f.atlas_rgba();
    let (w, h) = (f.atlas_width as usize, f.atlas_height as usize);
    println!("{p}: atlas {w}x{h} line={} glyphs={}", f.line_height, f.glyphs.len());
    let g = f.glyph('A').expect("A");
    println!("'A' at ({},{}) {}x{} off=({},{}) adv={}", g.x, g.y, g.width, g.height, g.offset_x, g.offset_y, g.advance);
    for y in g.y as usize..(g.y as usize + g.height as usize).min(h) {
        let row: String = (g.x as usize..(g.x as usize + g.width as usize).min(w))
            .map(|x| {
                let al = a[(y * w + x) * 4 + 3];
                if al > 200 { '#' } else if al > 100 { '+' } else if al > 0 { '.' } else { ' ' }
            })
            .collect();
        println!("|{row}|");
    }
}
