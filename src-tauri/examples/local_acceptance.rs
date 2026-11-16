//! Real local acceptance runner. Uses the production CLI and records actual
//! executed placements on stderr; data and outputs are supplied by the caller.
fn main() {
    eprintln!("RESOURCES {}", serde_json::to_string(&tifmrcview_lib::resources::inspect(None)).unwrap());
    tifmrcview_lib::compute::sink(std::sync::Arc::new(|p| {
        eprintln!("PLACEMENT {}", serde_json::to_string(&p).unwrap());
    }));
    std::process::exit(tifmrcview_lib::cli::main(std::env::args().skip(1).collect()));
}
