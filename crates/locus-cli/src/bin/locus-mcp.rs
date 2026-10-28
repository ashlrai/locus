//! Thin binary shim over the `locus-mcp` library crate.
//!
//! Ships inside the `locus-cli` package so one `cargo install locus-cli`
//! produces both `locus` and `locus-mcp` in a single build (issue #168).

fn main() {
    locus_mcp::main()
}
