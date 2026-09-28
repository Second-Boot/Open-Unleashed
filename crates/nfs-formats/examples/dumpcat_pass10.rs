fn main() {
    let path = std::env::args().nth(1).expect("nfs5.car path");
    let data = std::fs::read(&path).unwrap();
    let cats = nfs_formats::fedata::parse_car_catalog(&data).unwrap();
    for (i, c) in cats.iter().enumerate() {
        println!("{i} family={:?} sim_family={:?} class={}", c.family, c.sim_family, c.class_string_id);
    }
}
