use std::path::Path;
fn main() {
    let p = Path::new("/Users/eevv/focus/bun/test/js/bun/sqlite/sqlite.test.js");
    let r = bun_loader::prepare(p).unwrap();
    // Find lines that mention "db"
    for (i, line) in r.rewritten.lines().enumerate() {
        if line.contains(" db ") || line.contains(" db=") || line.starts_with("db ") {
            println!("{:4}: {}", i+1, line);
        }
    }
}
