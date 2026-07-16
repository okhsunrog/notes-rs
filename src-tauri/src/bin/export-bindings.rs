fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "src/lib/bindings.ts".to_owned());
    notes_rs_lib::export_bindings(&path)
        .unwrap_or_else(|error| panic!("could not export bindings to {path}: {error}"));
}
