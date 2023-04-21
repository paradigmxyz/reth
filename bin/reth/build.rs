fn main() {
    let mut opts = built::Options::default();
    opts.set_dependencies(true);

    built::write_built_file().expect("Failed to acquire build-time information");
}
