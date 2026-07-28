// A second package over the same source, because cargo names artifact
// dependency environment variables after the package: the harness needs the
// host build and the x86_64-unknown-linux-musl build of the target at once,
// and one package cannot be an artifact dependency for two triples.
include!("../../fspy_benchmark_target/src/main.rs");
