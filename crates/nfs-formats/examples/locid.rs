//! Diagnostic: `.loc` lookup by id, current language. `locid.rs <file> <lang> <id...>`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let d = std::fs::read(&a[0]).unwrap();
    let mut t = nfs_formats::loc::LocTable::parse(d).unwrap();
    let lang: u16 = a[1].parse().unwrap();
    t.set_language(lang);
    for id in &a[2..] {
        let id: u32 = id.parse().unwrap();
        match t.get(id) {
            Some(bytes) => {
                let s: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                println!("{id}: {bytes:?} -> {s:?}");
            }
            None => println!("{id}: None"),
        }
    }
}
