fn main() {
    let d = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let l = nfs_formats::loc::LocFile::parse(&d, 0).unwrap();
    for (i, e) in l.entries.iter().enumerate() {
        println!("{i:3} {e:?}");
    }
}
