fn main() {
    flatbed_build::Config::new()
        .schema("schemas/greet.fbs")
        .compile()
        .expect("flatbed codegen failed");
}
