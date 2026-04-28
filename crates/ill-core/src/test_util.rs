use crate::ast::Span;

/// A zero-length span, for tests that build AST nodes by hand and don't
/// care about source positions.
pub fn dummy_span() -> Span {
    Span { start: 0, end: 0 }
}

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
