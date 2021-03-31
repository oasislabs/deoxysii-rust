fn main() {
    cc::Build::new()
        .file("src/sanitize_xmm_registers.S")
        .compile("sanitize_xmm_registers");
}
