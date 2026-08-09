fn main() {
    // This library is injected into every traced process and runs inside every
    // intercepted libc call, so an unoptimized build is a silent, large
    // regression for whoever consumes it.
    artifact_profile::require_optimized_in_release();
}
