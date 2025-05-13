use crashlog::cargo_metadata;

fn main() {
    // For crash reporting
    crashlog::setup!(cargo_metadata!(), false);
}
