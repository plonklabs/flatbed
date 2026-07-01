fn main() {
    flatbed_build::Config::new()
        .schema("schemas/ping.fbs")
        .compile()
        .expect("flatbed codegen failed");
}
