//! Writes the MAD decoder's tables out of `nfs5.exe` into the small `madtables.bin` a self-contained
//! app bundle carries instead of the executable (`packaging/make_app.sh --with-data`).
//! Usage: `extract_mad_tables <nfs5.exe> <out/madtables.bin>`.
fn main() {
    let mut a = std::env::args().skip(1);
    let (Some(exe), Some(out)) = (a.next(), a.next()) else {
        eprintln!("usage: extract_mad_tables <nfs5.exe> <out/madtables.bin>");
        std::process::exit(2);
    };
    let data = std::fs::read(&exe).expect("read nfs5.exe");
    let t = nfs_formats::mad::MadTables::from_exe(&data).expect("MAD tables in nfs5.exe");
    let bytes = t.to_bytes();
    std::fs::write(&out, &bytes).expect("write madtables.bin");
    println!("{} bytes -> {out}", bytes.len());
}
