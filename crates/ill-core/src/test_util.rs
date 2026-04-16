/// Recursively collect all `.ill` files under `dir`.
pub fn collect_ill_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fn visit(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                visit(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("ill") {
                out.push(p);
            }
        }
    }
    let mut paths = Vec::new();
    visit(dir, &mut paths);
    paths.sort();
    paths
}
